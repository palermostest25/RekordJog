//! RekordJog core: a pure, platform-independent MIDI translation engine.
//!
//! The Numark Mixtrack Go is not supported by rekordbox, and rekordbox only
//! enables jog wheels for Pioneer hardware. RekordJog therefore presents a
//! virtual MIDI device called `PIONEER DDJ-SX` to rekordbox and translates
//! every control of the Mixtrack Go into the exact message a real DDJ-SX
//! would send. rekordbox then uses its own built-in DDJ-SX mapping, so no
//! mapping files ever need to be imported or replaced. LED messages that
//! rekordbox sends to the "DDJ-SX" are translated back to the Mixtrack Go.
//!
//! This crate contains no I/O at all: the app feeds raw MIDI bytes in and
//! gets raw MIDI bytes (tagged with a destination) back, which keeps it
//! fully unit-testable.

pub mod config;
pub mod ddj;
pub mod engine;
pub mod mixtrack;

pub use config::Config;
pub use engine::{Dest, Engine, Out, Snapshot, UiEvent};

use serde::{Deserialize, Serialize};

/// Which half of the controller a control belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub const BOTH: [Side; 2] = [Side::Left, Side::Right];

    pub fn idx(self) -> usize {
        match self {
            Side::Left => 0,
            Side::Right => 1,
        }
    }

    pub fn from_idx(i: usize) -> Side {
        if i == 0 {
            Side::Left
        } else {
            Side::Right
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Side::Left => "Left deck",
            Side::Right => "Right deck",
        }
    }
}

/// The physical behaviour of a control, used to match controls to actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Momentary button (note on / note off).
    Button,
    /// Absolute knob or fader (7-bit CC).
    Knob,
    /// Relative encoder (CC, 1 = clockwise, 127 = counter-clockwise).
    Encoder,
    /// Built-in behaviour that cannot be remapped (jog wheels, tempo, pads).
    Fixed,
}

/// Everything the UI needs to render the mapping editor.
#[derive(Clone, Debug, Serialize)]
pub struct Catalog {
    pub controls: Vec<ControlInfo>,
    pub actions: Vec<ddj::ActionInfo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ControlInfo {
    pub id: String,
    pub label: String,
    pub section: String,
    pub kind: Kind,
    pub side: Option<Side>,
    pub default_action: String,
}

/// Controls, actions and defaults for the mapping editor.
pub fn catalog() -> Catalog {
    let mut controls: Vec<ControlInfo> = mixtrack::controls()
        .into_iter()
        .map(|c| ControlInfo {
            id: c.id,
            label: c.label,
            section: c.section,
            kind: c.kind,
            side: c.side,
            default_action: c.default_action.to_string(),
        })
        .collect();
    for side in Side::BOTH {
        for (id, label, desc) in [
            ("jog", "Jog wheel", "scratch / pitch bend / search (built-in)"),
            ("tempo", "Tempo fader", "tempo (built-in)"),
            ("mode", "Mode button", "cycles pad modes (built-in)"),
            ("pads", "Performance pads 1-4", "follow the pad mode (built-in)"),
        ] {
            controls.push(ControlInfo {
                id: format!("{}.{}", side.key(), id),
                label: label.to_string(),
                section: side.title().to_string(),
                kind: Kind::Fixed,
                side: Some(side),
                default_action: desc.to_string(),
            });
        }
    }
    Catalog {
        controls,
        actions: ddj::actions().to_vec(),
    }
}
