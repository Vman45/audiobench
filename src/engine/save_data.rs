use crate::engine::parts as ep;
use crate::engine::registry::Registry;
use crate::util::*;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::PathBuf;

fn deserialize_string(reader: &mut io::BufReader<impl Read>, len: usize) -> io::Result<String> {
    let mut buffer = vec![0; len];
    reader.read_exact(&mut buffer[..])?;
    Ok(String::from_utf8(buffer).expect("TODO: Nice data corruption error."))
}

#[derive(Debug, Clone)]
struct SavedAutomationLane {
    module_index: usize,
    output_index: usize,
    range: (f32, f32),
}

impl SavedAutomationLane {
    fn serialize(&self, writer: &mut io::BufWriter<impl Write>) -> io::Result<()> {
        assert!(self.module_index < 0xFFFF);
        assert!(self.output_index < 0xFF);
        writer.write_all(&[
            (self.module_index >> 8) as u8,
            (self.module_index & 0xFF) as u8,
            self.output_index as u8,
        ])?;
        writer.write_all(&self.range.0.to_bits().to_be_bytes())?;
        writer.write_all(&self.range.1.to_bits().to_be_bytes())?;
        Ok(())
    }

    fn deserialize(reader: &mut io::BufReader<impl Read>) -> io::Result<Self> {
        let mut buffer = [0; 11];
        reader.read_exact(&mut buffer)?;
        Ok(Self {
            module_index: u16::from_be_bytes([buffer[0], buffer[1]]) as usize,
            output_index: buffer[2] as usize,
            range: (
                f32::from_bits(u32::from_be_bytes([
                    buffer[3], buffer[4], buffer[5], buffer[6],
                ])),
                f32::from_bits(u32::from_be_bytes([
                    buffer[7], buffer[8], buffer[9], buffer[10],
                ])),
            ),
        })
    }
}

#[derive(Debug, Clone)]
struct SavedControl {
    value: f32,
    automation_lanes: Vec<SavedAutomationLane>,
}

impl SavedControl {
    fn serialize(&self, writer: &mut io::BufWriter<impl Write>) -> io::Result<()> {
        writer.write_all(&self.value.to_bits().to_be_bytes())?;
        assert!(self.automation_lanes.len() <= 0xFF);
        writer.write_all(&[self.automation_lanes.len() as u8])?;
        for lane in &self.automation_lanes {
            lane.serialize(writer)?;
        }
        Ok(())
    }

    fn deserialize(reader: &mut io::BufReader<impl Read>) -> io::Result<Self> {
        let mut buffer = [0; 5];
        reader.read_exact(&mut buffer)?;
        let value = f32::from_bits(u32::from_be_bytes([
            buffer[0], buffer[1], buffer[2], buffer[3],
        ]));
        let num_lanes = buffer[4] as usize;
        let automation_lanes = (0..num_lanes)
            .map(|_| SavedAutomationLane::deserialize(reader))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            value,
            automation_lanes,
        })
    }
}

#[derive(Debug, Clone)]
struct SavedComplexControl {
    value: String,
}

impl SavedComplexControl {
    fn serialize(&self, writer: &mut io::BufWriter<impl Write>) -> io::Result<()> {
        let value = self.value.as_bytes();
        assert!(value.len() <= 0xFFFF);
        writer.write_all(&[(value.len() >> 8) as u8, (value.len() & 0xFF) as u8])?;
        writer.write_all(&value)?;
        Ok(())
    }

    fn deserialize(reader: &mut io::BufReader<impl Read>) -> io::Result<Self> {
        let mut buffer = [0; 2];
        reader.read_exact(&mut buffer)?;
        let value_size = u16::from_be_bytes([buffer[0], buffer[1]]) as usize;
        let value = deserialize_string(reader, value_size)?;
        Ok(Self { value })
    }
}

#[derive(Debug, Clone)]
enum SavedInputConnection {
    Default(usize),
    Output {
        module_index: usize,
        output_index: usize,
    },
}

impl SavedInputConnection {
    fn serialize(&self, writer: &mut io::BufWriter<impl Write>) -> io::Result<()> {
        match self {
            Self::Default(index) => {
                assert!(*index <= 0xFF);
                writer.write_all(&[0xFF, 0x00, *index as u8])?;
            }
            Self::Output {
                module_index,
                output_index,
            } => {
                assert!(*module_index <= 0x7FFF);
                assert!(*output_index <= 0xFF);
                writer.write_all(&[
                    (*module_index >> 8) as u8,
                    (*module_index & 0xFF) as u8,
                    *output_index as u8,
                ])?;
            }
        }
        Ok(())
    }

    fn deserialize(reader: &mut io::BufReader<impl Read>) -> io::Result<Self> {
        let mut buffer = [0; 3];
        reader.read_exact(&mut buffer)?;
        Ok(if buffer[0] == 0xFF {
            Self::Default(buffer[2] as usize)
        } else {
            Self::Output {
                module_index: u16::from_be_bytes([buffer[0], buffer[1]]) as usize,
                output_index: buffer[2] as usize,
            }
        })
    }
}

#[derive(Debug, Clone)]
struct SavedModule {
    resource_name: String,
    controls: Vec<SavedControl>,
    complex_controls: Vec<SavedComplexControl>,
    input_connections: Vec<SavedInputConnection>,
    pos: (i32, i32),
}

impl SavedModule {
    fn restore(&self, registry: &Registry) -> Result<ep::Module, String> {
        let mut module = registry
            .borrow_module(&self.resource_name)
            .ok_or_else(|| {
                format!(
                    "Error encountered loading patch: Could not find a module named {}",
                    &self.resource_name
                )
            })?
            .clone();
        if module.controls.len() != self.controls.len() {
            return Err(format!(
                "Corrupt preset: The number of controls in {} has changed",
                &self.resource_name
            ));
        }
        for index in 0..self.controls.len() {
            module.controls[index].borrow_mut().value = self.controls[index].value;
        }
        if module.complex_controls.len() != self.complex_controls.len() {
            return Err(format!(
                "Corrupt preset: The number of complex controls in {} has changed",
                &self.resource_name
            ));
        }
        for index in 0..self.complex_controls.len() {
            module.complex_controls[index].borrow_mut().value =
                self.complex_controls[index].value.clone();
        }
        module.pos = self.pos;
        Ok(module)
    }

    fn restore_connections(
        &self,
        on: &mut ep::Module,
        modules: &[Rcrc<ep::Module>],
    ) -> Result<(), String> {
        if on.inputs.len() != self.input_connections.len() {
            return Err(format!(
                "Corrupt preset: The number of inputs in {} has changed",
                &self.resource_name
            ));
        }
        for index in 0..self.input_connections.len() {
            on.inputs[index] = match &self.input_connections[index] {
                SavedInputConnection::Default(index) => ep::InputConnection::Default(*index),
                SavedInputConnection::Output {
                    module_index,
                    output_index,
                } => {
                    if *module_index >= modules.len() {
                        return Err(format!("Corrupt preset: Module index out of bounds"));
                    }
                    let module = &modules[*module_index];
                    let module_ref = module.borrow();
                    let out_temp_ref = module_ref.template.borrow();
                    if out_temp_ref.outputs.len() <= *output_index {
                        return Err(format!("Corrupt preset: Output index out of bounds"));
                    }
                    if out_temp_ref.outputs[*output_index].get_type()
                        != on.template.borrow().inputs[index].get_type()
                    {
                        return Err(format!("Corrupt preset: Wire has mismatched data types"));
                    }
                    drop(out_temp_ref);
                    drop(module_ref);
                    ep::InputConnection::Wire(Rc::clone(module), *output_index)
                }
            };
        }
        for index in 0..self.controls.len() {
            let mut control_ref = on.controls[index].borrow_mut();
            for lane in &self.controls[index].automation_lanes {
                if lane.module_index >= modules.len() {
                    return Err(format!("Corrupt preset: Module index out of bounds"));
                }
                let module = Rc::clone(&modules[lane.module_index]);
                if module.borrow().template.borrow().outputs.len() <= lane.output_index {
                    return Err(format!("Corrupt preset: Output index out of bounds"));
                }
                if module.borrow().template.borrow().outputs[lane.output_index].get_type()
                    != ep::JackType::Audio
                {
                    return Err(format!(
                        "Corrupt preset: Automation wire is not connected to audio jack"
                    ));
                }
                control_ref.automation.push(ep::AutomationLane {
                    connection: (module, lane.output_index),
                    range: lane.range,
                });
            }
        }
        Ok(())
    }

    fn serialize(&self, writer: &mut io::BufWriter<impl Write>) -> io::Result<()> {
        let resource_name = self.resource_name.as_bytes();

        writer.write_all(&self.pos.0.to_be_bytes())?;
        writer.write_all(&self.pos.1.to_be_bytes())?;
        assert!(resource_name.len() <= 0xFF);
        writer.write_all(&[resource_name.len() as u8])?;
        assert!(self.controls.len() <= 0xFF);
        writer.write_all(&[self.controls.len() as u8])?;
        assert!(self.complex_controls.len() <= 0xFF);
        writer.write_all(&[self.complex_controls.len() as u8])?;
        assert!(self.input_connections.len() <= 0xFF);
        writer.write_all(&[self.input_connections.len() as u8])?;

        writer.write_all(resource_name)?;
        for control in &self.controls {
            control.serialize(writer)?;
        }
        for complex_control in &self.complex_controls {
            complex_control.serialize(writer)?;
        }
        for input_connection in &self.input_connections {
            input_connection.serialize(writer)?;
        }
        Ok(())
    }

    fn deserialize(reader: &mut io::BufReader<impl Read>) -> io::Result<Self> {
        let mut buffer = [0; 12];
        reader.read_exact(&mut buffer)?;
        let x = i32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
        let y = i32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);
        let resource_name_len = buffer[8] as usize;
        let controls_len = buffer[9] as usize;
        let complex_controls_len = buffer[10] as usize;
        let input_connections_len = buffer[11] as usize;
        let resource_name = deserialize_string(reader, resource_name_len)?;
        let controls = (0..controls_len)
            .map(|_| SavedControl::deserialize(reader))
            .collect::<Result<_, _>>()?;
        let complex_controls = (0..complex_controls_len)
            .map(|_| SavedComplexControl::deserialize(reader))
            .collect::<Result<_, _>>()?;
        let input_connections = (0..input_connections_len)
            .map(|_| SavedInputConnection::deserialize(reader))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            resource_name,
            controls,
            complex_controls,
            input_connections,
            pos: (x, y),
        })
    }
}

#[derive(Debug, Clone)]
struct SavedModuleGraph {
    modules: Vec<SavedModule>,
}

impl SavedModuleGraph {
    fn save_control(
        control: &Rcrc<ep::Control>,
        module_indexes: &HashMap<*const RefCell<ep::Module>, usize>,
    ) -> SavedControl {
        let control_ref = control.borrow();
        let value = control_ref.value;
        let automation_lanes = control_ref
            .automation
            .iter()
            .map(|lane| {
                let module_index = *module_indexes
                    .get(&(&*lane.connection.0.as_ref() as *const _))
                    .unwrap();
                let output_index = lane.connection.1;
                let range = lane.range;
                SavedAutomationLane {
                    module_index,
                    output_index,
                    range,
                }
            })
            .collect();
        SavedControl {
            value,
            automation_lanes,
        }
    }

    fn save_input(
        input: &ep::InputConnection,
        module_indexes: &HashMap<*const RefCell<ep::Module>, usize>,
    ) -> SavedInputConnection {
        match input {
            ep::InputConnection::Wire(module, output_index) => SavedInputConnection::Output {
                module_index: *module_indexes
                    .get(&(&*module.as_ref() as *const _))
                    .unwrap(),
                output_index: *output_index,
            },
            ep::InputConnection::Default(default_index) => {
                SavedInputConnection::Default(*default_index)
            }
        }
    }

    fn save_module(
        module: &Rcrc<ep::Module>,
        module_indexes: &HashMap<*const RefCell<ep::Module>, usize>,
    ) -> SavedModule {
        let mod_ref = module.borrow();
        let template_ref = mod_ref.template.borrow();
        let resource_name = template_ref.resource_name.clone();
        let controls = mod_ref
            .controls
            .iter()
            .map(|control| Self::save_control(control, module_indexes))
            .collect();
        let complex_controls = mod_ref
            .complex_controls
            .iter()
            .map(|control| SavedComplexControl {
                value: control.borrow().value.clone(),
            })
            .collect();
        let input_connections = mod_ref
            .inputs
            .iter()
            .map(|input| Self::save_input(input, module_indexes))
            .collect();
        let pos = mod_ref.pos;
        SavedModule {
            resource_name,
            controls,
            complex_controls,
            input_connections,
            pos,
        }
    }

    fn save(graph: &ep::ModuleGraph) -> Self {
        let mut module_indexes: HashMap<*const RefCell<ep::Module>, usize> = HashMap::new();
        for (index, module) in graph.borrow_modules().iter().enumerate() {
            module_indexes.insert(&*module.as_ref(), index);
        }
        let module_indexes = module_indexes;
        let modules = graph
            .borrow_modules()
            .iter()
            .map(|module| Self::save_module(module, &module_indexes))
            .collect();
        Self { modules }
    }

    fn blank() -> Self {
        Self {
            modules: Default::default(),
        }
    }

    fn restore(&self, graph: &mut ep::ModuleGraph, registry: &Registry) -> Result<(), String> {
        let modules: Vec<_> = self
            .modules
            .iter()
            .map(|m| m.restore(registry).map(|m| rcrc(m)))
            .collect::<Result<_, _>>()?;
        for index in 0..self.modules.len() {
            let module = Rc::clone(&modules[index]);
            self.modules[index].restore_connections(&mut *module.borrow_mut(), &modules[..])?;
        }
        graph.set_modules(modules);
        Ok(())
    }

    fn serialize(&self, writer: &mut io::BufWriter<impl Write>) -> io::Result<()> {
        // Indexes <= 0x3FF
        assert!(self.modules.len() <= 0x400);
        writer.write_all(&(self.modules.len() as u16).to_be_bytes())?;
        for module in &self.modules {
            module.serialize(writer)?;
        }
        Ok(())
    }

    fn deserialize(reader: &mut io::BufReader<impl Read>) -> io::Result<Self> {
        let mut buffer = [0; 2];
        reader.read_exact(&mut buffer)?;
        let num_modules = u16::from_be_bytes([buffer[0], buffer[1]]) as usize;
        let modules = (0..num_modules)
            .map(|_| SavedModule::deserialize(reader))
            .collect::<Result<_, _>>()?;
        Ok(Self { modules })
    }
}

#[derive(Debug, Clone)]
enum PatchSource {
    Writable(PathBuf),
    Readable(String),
}

#[derive(Debug, Clone)]
pub struct Patch {
    source: PatchSource,
    name: String,
    note_graph: SavedModuleGraph,
}

impl Patch {
    pub fn writable(save_path: PathBuf) -> Self {
        Self {
            name: "Unnamed".to_owned(),
            note_graph: SavedModuleGraph::blank(),
            source: PatchSource::Writable(save_path),
        }
    }

    pub fn load_readable(
        source: String,
        reader: &mut io::BufReader<impl Read>,
    ) -> io::Result<Self> {
        Self::deserialize(PatchSource::Readable(source), reader)
    }

    pub fn load_writable(
        source: PathBuf,
        reader: &mut io::BufReader<impl Read>,
    ) -> io::Result<Self> {
        Self::deserialize(PatchSource::Writable(source), reader)
    }

    pub fn is_writable(&self) -> bool {
        if let PatchSource::Writable(..) = &self.source {
            true
        } else {
            false
        }
    }

    pub fn set_name(&mut self, name: String) {
        self.name = name;
    }

    pub fn borrow_name(&self) -> &str {
        &self.name
    }

    pub fn save_note_graph(&mut self, graph: &ep::ModuleGraph) {
        self.note_graph = SavedModuleGraph::save(graph);
    }

    pub fn restore_note_graph(
        &self,
        graph: &mut ep::ModuleGraph,
        registry: &Registry,
    ) -> Result<(), String> {
        self.note_graph.restore(graph, registry)
    }

    pub fn write(&self) -> io::Result<()> {
        let path = if let PatchSource::Writable(path) = &self.source {
            path
        } else {
            panic!("Cannot write a non-writable patch!");
        };
        let file = std::fs::File::create(path)?;
        let mut writer = std::io::BufWriter::new(file);
        self.serialize(&mut writer)?;
        Ok(())
    }

    fn serialize(&self, writer: &mut io::BufWriter<impl Write>) -> io::Result<()> {
        let name = self.name.as_bytes();
        assert!(name.len() < 0xFFFF);
        writer.write_all(&(name.len() as u16).to_be_bytes())?;
        writer.write_all(name)?;
        self.note_graph.serialize(writer)?;
        Ok(())
    }

    fn deserialize(source: PatchSource, reader: &mut io::BufReader<impl Read>) -> io::Result<Self> {
        let mut buffer = [0; 2];
        reader.read_exact(&mut buffer)?;
        let name_size = u16::from_be_bytes([buffer[0], buffer[1]]) as usize;
        let name = deserialize_string(reader, name_size)?;
        let note_graph = SavedModuleGraph::deserialize(reader)?;
        Ok(Self {
            source,
            name,
            note_graph,
        })
    }
}