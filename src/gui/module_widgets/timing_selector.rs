use super::ModuleWidget;
use crate::engine::parts as ep;
use crate::engine::registry::Registry;
use crate::gui::action::{MouseAction};
use crate::gui::constants::*;
use crate::gui::graphics::{GrahpicsWrapper, HAlign, VAlign};
use crate::gui::{InteractionHint, MouseMods, Tooltip};
use crate::util::*;

#[derive(Clone)]
pub struct TimingSelector {
    source_control: Rcrc<ep::ComplexControl>,
    type_control: Rcrc<ep::ComplexControl>,
    pos: (f32, f32),
    note_icon: usize,
    song_icon: usize,
    time_icon: usize,
    beats_icon: usize,
}

impl TimingSelector {
    pub(super) fn create(
        source_control: Rcrc<ep::ComplexControl>,
        type_control: Rcrc<ep::ComplexControl>,
        pos: (f32, f32),
        registry: &Registry,
    ) -> Self {
        Self {
            source_control,
            type_control,
            pos,
            note_icon: registry.lookup_icon("base:note").unwrap(),
            song_icon: registry.lookup_icon("base:treble_clef").unwrap(),
            time_icon: registry.lookup_icon("base:time").unwrap(),
            beats_icon: registry.lookup_icon("base:metronome").unwrap(),
        }
    }

    fn source_value(&self) -> bool {
        self.source_control.borrow().value == "TRUE"
    }

    fn type_value(&self) -> bool {
        self.type_control.borrow().value == "TRUE"
    }
}

impl ModuleWidget for TimingSelector {
    fn get_position(&self) -> (f32, f32) {
        self.pos
    }

    fn get_bounds(&self) -> (f32, f32) {
        (grid(2), grid(2))
    }

    fn respond_to_mouse_press(
        &self,
        local_pos: (f32, f32),
        mods: &MouseMods,
        parent_pos: (f32, f32),
    ) -> MouseAction {
        if local_pos.0 < grid(2) / 2.0 {
            MouseAction::SetComplexControl(
                Rc::clone(&self.source_control),
                if self.source_value() { "FALSE" } else { "TRUE" }.to_owned(),
            )
        } else {
            MouseAction::SetComplexControl(
                Rc::clone(&self.type_control),
                if self.type_value() { "FALSE" } else { "TRUE" }.to_owned(),
            )
        }
    }

    fn get_tooltip_at(&self, local_pos: (f32, f32)) -> Option<Tooltip> {
        Some(Tooltip {
            text: if local_pos.0 < grid(2) / 2.0 {
                format!(
                    "Change timing source, current value is \"{}\"",
                    if self.source_value() { "song" } else { "note" }
                )
            } else {
                format!(
                    "Change timing type, current value is \"{}\"",
                    if self.type_value() {
                        "beats"
                    } else {
                        "seconds"
                    }
                )
            },
            interaction: InteractionHint::LeftClick.into(),
        })
    }

    fn draw(
        &self,
        g: &mut GrahpicsWrapper,
        highlight: bool,
        parent_pos: (f32, f32),
        feedback_data: &[f32],
    ) {
        g.push_state();
        g.apply_offset(self.pos.0, self.pos.1);

        const CS: f32 = CORNER_SIZE;
        const ICON_SIZE: f32 = (grid(2) - CS * 3.0) / 2.0;
        g.set_color(&COLOR_BG);
        g.fill_rounded_rect(0.0, 0.0, grid(2), CS * 2.0 + ICON_SIZE, CS);
        g.draw_white_icon(
            if self.source_value() {
                self.song_icon
            } else {
                self.note_icon
            },
            CS,
            CS,
            ICON_SIZE,
        );
        g.draw_white_icon(
            if self.type_value() {
                self.beats_icon
            } else {
                self.time_icon
            },
            CS + ICON_SIZE + CS,
            CS,
            ICON_SIZE,
        );
        g.set_color(&COLOR_TEXT);
        g.write_text(
            FONT_SIZE,
            0.0,
            0.0,
            grid(2),
            grid(2),
            HAlign::Center,
            VAlign::Bottom,
            1,
            "Timing",
        );

        g.pop_state();
    }
}