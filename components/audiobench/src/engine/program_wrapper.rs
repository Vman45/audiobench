use crate::engine::data_transfer::{DataFormat, HostData, InputPacker, NoteData, OutputUnpacker};
use crate::registry::Registry;
use array_macro::array;
use nodespeak::llvmir::structure::{Program, StaticData};
use nodespeak::Compiler;
use shared_util::{perf_counter::sections, prelude::*};

/// The MIDI protocol can provide notes at 128 different pitches.
const NUM_MIDI_NOTES: usize = 128;
/// Notes must be silent for at least this amount of time before they will be shut off.
const MIN_SILENT_TIME: f32 = 0.1;
/// Notes must have every sample be of this magnitude or less to be considered silent.
const SILENT_CUTOFF: f32 = 1e-5;

struct CompleteNoteData {
    input_data: NoteData,
    silent_samples: usize,
    static_data: StaticData,
}

pub struct NoteTracker {
    held_notes: [Option<CompleteNoteData>; NUM_MIDI_NOTES],
    decaying_notes: Vec<CompleteNoteData>,
    data_format: DataFormat,
}

impl NoteTracker {
    pub fn new(data_format: DataFormat) -> Self {
        Self {
            held_notes: array![None; NUM_MIDI_NOTES],
            decaying_notes: Vec::new(),
            data_format,
        }
    }

    pub fn silence_all(&mut self) {
        self.held_notes = array![None; NUM_MIDI_NOTES];
        self.decaying_notes.clear();
    }

    pub fn set_data_format(&mut self, data_format: DataFormat) {
        self.data_format = data_format;
        // Old notes may not be compatible with the new data data_format.
        self.silence_all();
    }

    fn equal_tempered_tuning(index: usize) -> f32 {
        // MIDI note 69 is 440Hz. 12 notes is an octave (double / half frequency).
        440.0 * (2.0f32).powf((index as i32 - 69) as f32 / 12.0)
    }

    pub fn start_note(&mut self, static_data: StaticData, index: usize, velocity: f32) {
        if self.held_notes[index].is_some() {
            return;
        }
        self.held_notes[index] = Some(CompleteNoteData {
            input_data: NoteData {
                pitch: Self::equal_tempered_tuning(index),
                velocity,
                elapsed_samples: 0,
                elapsed_beats: 0.0,
                start_trigger: true,
                release_trigger: false,
            },
            silent_samples: 0,
            static_data,
        });
    }

    pub fn release_note(&mut self, index: usize) {
        if let Some(mut note) = self.held_notes[index].take() {
            note.input_data.start_trigger = false;
            note.input_data.release_trigger = true;
            self.decaying_notes.push(note);
        }
    }

    fn advance_all_notes(&mut self, host_data: &HostData) {
        let sample_rate = self.data_format.host_format.sample_rate as f32;
        let buffer_len = self.data_format.host_format.buffer_len;
        let min_silent_samples = (MIN_SILENT_TIME * sample_rate) as usize;
        let buffer_beats = host_data.bpm / 60.0 * buffer_len as f32 / sample_rate;
        for index in (0..self.decaying_notes.len()).rev() {
            let note = &mut self.decaying_notes[index];
            if note.silent_samples >= min_silent_samples {
                self.decaying_notes.remove(index);
            } else {
                note.input_data.elapsed_samples += buffer_len;
                note.input_data.elapsed_beats += buffer_beats;
                note.input_data.start_trigger = false;
                note.input_data.release_trigger = false;
            }
        }
        for note in self.held_notes.iter_mut().filter_map(|o| o.as_mut()) {
            note.input_data.elapsed_samples += buffer_len;
            note.input_data.elapsed_beats += buffer_beats;
            note.input_data.start_trigger = false;
        }
    }

    fn recommend_note_for_feedback(&self) -> Option<usize> {
        let mut youngest_time = std::usize::MAX;
        for note in self.held_notes.iter().filter_map(|o| o.as_ref()) {
            youngest_time = youngest_time.min(note.input_data.elapsed_samples);
        }
        // If there are no held notes, it is okay to display a decaying note insteaad.
        if youngest_time == std::usize::MAX {
            for note in &self.decaying_notes {
                youngest_time = youngest_time.min(note.input_data.elapsed_samples);
            }
        }
        let mut index = 0;
        for note in self.held_notes.iter().filter_map(|o| o.as_ref()) {
            if note.input_data.elapsed_samples == youngest_time {
                return Some(index);
            }
            index += 1;
        }
        for note in &self.decaying_notes {
            if note.input_data.elapsed_samples == youngest_time {
                return Some(index);
            }
            index += 1;
        }
        None
    }

    fn active_notes_mut(&mut self) -> impl Iterator<Item = &mut CompleteNoteData> {
        let held_iter = self.held_notes.iter_mut().filter_map(|o| o.as_mut());
        let decaying_iter = self.decaying_notes.iter_mut();
        held_iter.chain(decaying_iter)
    }
}

pub(super) struct AudiobenchCompiler {
    compiler: Compiler,
}

impl AudiobenchCompiler {
    pub(super) fn new(registry: &Registry) -> Self {
        let mut compiler = Compiler::new();
        compiler.add_source("<note graph>".to_owned(), "".to_owned());
        compiler.add_source("!:lib.ns".to_owned(), include_str!("lib.ns").to_owned());
        for (name, content) in registry.borrow_scripts().iter() {
            compiler.add_source(name.to_owned(), content.to_owned());
        }
        Self { compiler }
    }

    pub(super) fn compile(
        &mut self,
        source: String,
        data_format: DataFormat,
    ) -> Result<AudiobenchProgram, String> {
        self.compiler.add_source("<note graph>".to_owned(), source);
        Ok(AudiobenchProgram {
            program: self.compiler.compile("<note graph>")?,
            data_format,
        })
    }

    pub(super) fn reset_performance_counters(&mut self) {
        self.compiler.reset_performance_counters();
    }

    pub(super) fn tally_performance_counters(&mut self, global_counter: &mut impl PerfCounter) {
        let compiler_counters = self.compiler.borrow_performance_counters();
        global_counter.add_externally_timed_section(
            &sections::COMPILER_AST_PHASE,
            compiler_counters.ast.get_total_time().clone(),
        );
        global_counter.add_externally_timed_section(
            &sections::COMPILER_VAGUE_PHASE,
            compiler_counters.vague.get_total_time().clone(),
        );
        global_counter.add_externally_timed_section(
            &sections::COMPILER_RESOLVED_PHASE,
            compiler_counters.resolved.get_total_time().clone(),
        );
        global_counter.add_externally_timed_section(
            &sections::COMPILER_TRIVIAL_PHASE,
            compiler_counters.trivial.get_total_time().clone(),
        );
        global_counter.add_externally_timed_section(
            &sections::COMPILER_LLVMIR_PHASE,
            compiler_counters.llvmir.get_total_time().clone(),
        );
        self.compiler.reset_performance_counters();
    }
}

pub struct AudiobenchProgram {
    program: Program,
    data_format: DataFormat,
}

impl AudiobenchProgram {
    pub fn create_static_data(&mut self) -> Result<StaticData, String> {
        unsafe { self.program.create_static_data().map_err(|e| e.to_owned()) }
    }

    pub fn get_input_packer(&mut self) -> InputPacker {
        InputPacker::new(self.program.borrow_input_packer_mut(), &self.data_format)
    }

    pub fn get_output_unpacker(&self) -> OutputUnpacker {
        OutputUnpacker::new(self.program.borrow_output_unpacker())
    }

    /// This handles everything from global setup, note iteration, program execution, note teardown,
    /// and finally global teardown. Returns true if feedback data was updated.
    pub fn execute(
        &mut self,
        update_feedback: bool,
        host_data: &mut HostData,
        notes: &mut NoteTracker,
        audio_output: &mut [f32],
        perf_counter: &mut impl PerfCounter,
    ) -> Result<bool, String> {
        let section = perf_counter.begin_section(&sections::GLOBAL_SETUP);
        let buf_len = self.data_format.host_format.buffer_len;
        let sample_rate = self.data_format.host_format.sample_rate;
        let buf_time = buf_len as f32 / sample_rate as f32;
        assert!(audio_output.len() == buf_len * 2);

        for i in 0..buf_len * 2 {
            audio_output[i] = 0.0;
        }
        self.get_input_packer().set_host_data(&host_data);
        let feedback_note = if update_feedback {
            notes.recommend_note_for_feedback()
        } else {
            None
        };
        perf_counter.end_section(section);

        for (index, note) in notes.active_notes_mut().enumerate() {
            let section = perf_counter.begin_section(&sections::NOTE_SETUP);
            self.get_input_packer().set_note_data(
                &note.input_data,
                host_data,
                feedback_note == Some(index),
            );
            perf_counter.end_section(section);

            let section = perf_counter.begin_section(&sections::NODESPEAK_EXEC);
            unsafe {
                self.program.execute(&mut note.static_data)?;
            }
            perf_counter.end_section(section);

            let section = perf_counter.begin_section(&sections::NOTE_FINALIZE);
            let mut silent = true;
            let output = self.get_output_unpacker();
            let audio = output.borrow_audio_out();
            for i in 0..buf_len * 2 {
                audio_output[i] += audio[i];
                silent &= audio[i].abs() < SILENT_CUTOFF;
            }
            if silent {
                note.silent_samples += buf_len;
            } else {
                note.silent_samples = 0;
            }
            perf_counter.end_section(section);
        }

        let section = perf_counter.begin_section(&sections::GLOBAL_FINALIZE);
        notes.advance_all_notes(host_data);
        host_data.song_time += buf_time;
        host_data.song_beats += buf_time * host_data.bpm / 60.0;
        perf_counter.end_section(section);
        Ok(feedback_note.is_some())
    }
}
