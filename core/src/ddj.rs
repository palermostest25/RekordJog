//! Pioneer DDJ-SX MIDI messages as understood by rekordbox's built-in
//! `PIONEER DDJ-SX.midi.csv`, plus the catalogue of actions a Mixtrack Go
//! control can be bound to.

use crate::{Kind, Side};
use serde::Serialize;

/// Name of the virtual MIDI device rekordbox must see.
pub const PORT_NAME: &str = "PIONEER DDJ-SX";

// Deck channel (status nibble = deck index)
pub const JOG_SCRATCH: u8 = 0x22;
pub const JOG_PITCH_BEND: u8 = 0x23;
pub const JOG_SEARCH: u8 = 0x1F;
pub const WHEEL_PITCH_BEND: u8 = 0x21;
pub const WHEEL_SEARCH: u8 = 0x26;
pub const JOG_TOUCH: u8 = 0x36;
pub const TEMPO_MSB: u8 = 0x00;
pub const TEMPO_LSB: u8 = 0x20;
pub const PLAY: u8 = 0x0B;
pub const SHIFT: u8 = 0x3F;
pub const VINYL_STATE: u8 = 0x3A;

// Mixer channel (7) and pads (channel 8 + deck)
pub const MIXER_STATUS_NOTE: u8 = 0x96;
pub const MIXER_STATUS_CC: u8 = 0xB6;
pub const CROSSFADER: u8 = 0x1F;
pub const CFX_BASE: u8 = 0x17;
pub const EQ_LOW: u8 = 0x0F;
pub const PAD_CHANNEL_BASE: u8 = 7;
pub const LOADED_INDICATOR_STATUS: u8 = 0x9B;

pub const HIRES_CENTER: u16 = 8192;
pub const HIRES_MAX: u16 = 16383;

/// A rekordbox pad mode the pads can be in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PadMode {
    pub id: &'static str,
    pub name: &'static str,
    /// first DDJ-SX pad note for this mode; +8 is the shifted pad
    pub base: u8,
    /// DDJ-SX pad mode button note (deck channel) that switches rekordbox
    /// to the same mode. `None` for modes RekordJog handles itself.
    pub mode_note: Option<u8>,
    /// Pads make 1/2/4/8-beat loops (handled by RekordJog, because the
    /// DDJ-SX profile in rekordbox has no beat-loop pad mode).
    pub beat_loop: bool,
}

const fn pm(id: &'static str, name: &'static str, base: u8, note: u8) -> PadMode {
    PadMode { id, name, base, mode_note: Some(note), beat_loop: false }
}

/// Every pad mode that can be put on a MODE-light slot.
pub const PAD_MODE_KINDS: &[PadMode] = &[
    pm("hot_cue", "Hot Cue", 0x00, 0x1B),
    PadMode { id: "loops", name: "Loops", base: 0x00, mode_note: None, beat_loop: true },
    pm("pad_fx1", "Pad FX", 0x10, 0x1E),
    pm("pad_fx2", "Pad FX 2", 0x50, 0x6B),
    pm("beat_jump", "Beat Jump", 0x40, 0x69),
    pm("sampler", "Sampler", 0x30, 0x22),
    pm("velocity_sampler", "Velocity Sampler", 0x70, 0x41),
    pm("slicer", "Slicer", 0x20, 0x20),
    pm("slicer_loop", "Slicer Loop", 0x60, 0x6D),
];

pub fn pad_mode(id: &str) -> Option<&'static PadMode> {
    PAD_MODE_KINDS.iter().find(|m| m.id == id)
}

/// The MODE button steps through these slots in order, lighting the
/// Mixtrack Go's mode LEDs (0 HOT CUE, 1 LOOPS, 2 SAMPLER, 3 STEMS).
/// Each slot can hold any pad mode, or be switched off.
pub const PAD_SLOTS: &[(&str, &[usize])] = &[
    ("HOT CUE light", &[0]),
    ("LOOPS light", &[1]),
    ("FX lights (LOOPS + SAMPLER)", &[1, 2]),
    ("SAMPLER light", &[2]),
    ("STEMS light", &[3]),
];

pub const DEFAULT_PAD_SLOTS: [&str; 5] = ["hot_cue", "loops", "pad_fx1", "sampler", "pad_fx2"];

// Loop buttons on the deck channel
pub const AUTO_LOOP: u8 = 0x14;
pub const LOOP_HALF: u8 = 0x12;
pub const LOOP_DOUBLE: u8 = 0x13;
/// Loop sizes are tracked as log2(beats); rekordbox starts auto loops at 4 beats.
pub const DEFAULT_LOOP_LOG2: i8 = 2;

/// Behaviours handled by RekordJog itself rather than by rekordbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppAction {
    ToggleVinyl,
    ToggleKnobMode,
    ToggleFadeFx,
    NextPadMode,
}

/// What a control does once resolved for a specific deck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    None,
    /// DDJ-SX button. `led` is the rekordbox output that drives its light.
    Note { status: u8, note: u8, led: Option<(u8, u8)> },
    /// DDJ-SX 14-bit knob/fader (MSB on `cc`, LSB on `cc + 0x20`).
    HiRes { status: u8, cc: u8 },
    /// DDJ-SX 7-bit knob.
    Cc { status: u8, cc: u8 },
    /// DDJ-SX relative encoder.
    Rotary { status: u8, cc: u8 },
    /// Crossfader, with Fade FX handled by the engine.
    Crossfader,
    /// Color FX filter or EQ low, switchable at runtime.
    FilterOrLow,
    App(AppAction),
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct ActionInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub kind: Kind,
    /// Needs a deck (only valid on controls that belong to a side).
    pub per_deck: bool,
}

macro_rules! a {
    ($id:expr, $label:expr, $group:expr, $kind:ident, $deck:expr) => {
        ActionInfo { id: $id, label: $label, group: $group, kind: Kind::$kind, per_deck: $deck }
    };
}

const CURATED: &[ActionInfo] = &[
    a!("none", "Do nothing", "General", Button, false),
    a!("play", "Play / Pause", "Deck", Button, true),
    a!("play_shift", "Stutter (Shift + Play)", "Deck", Button, true),
    a!("cue", "Cue", "Deck", Button, true),
    a!("jump_start", "Jump to track start", "Deck", Button, true),
    a!("sync", "Beat Sync", "Deck", Button, true),
    a!("master", "Set as tempo master", "Deck", Button, true),
    a!("keylock", "Key Lock (Master Tempo)", "Deck", Button, true),
    a!("tempo_range", "Cycle tempo range", "Deck", Button, true),
    a!("slip", "Slip mode", "Deck", Button, true),
    a!("slip_reverse", "Slip reverse (censor)", "Deck", Button, true),
    a!("reverse", "Reverse", "Deck", Button, true),
    a!("load", "Load selected track", "Deck", Button, true),
    a!("headphone_cue", "Headphone cue", "Deck", Button, true),
    a!("tap_bpm", "Tap BPM", "Deck", Button, true),
    a!("grid_adjust", "Beat grid adjust", "Deck", Button, true),
    a!("grid_slide", "Beat grid slide", "Deck", Button, true),
    a!("autoloop", "4-beat auto loop", "Loop", Button, true),
    a!("loop_active", "Reloop / loop on-off", "Loop", Button, true),
    a!("loop_half", "Loop length ÷2", "Loop", Button, true),
    a!("loop_double", "Loop length ×2", "Loop", Button, true),
    a!("loop_in", "Loop in", "Loop", Button, true),
    a!("loop_out", "Loop out", "Loop", Button, true),
    a!("reloop_exit", "Reloop / exit", "Loop", Button, true),
    a!("loop_move_left", "Move loop left", "Loop", Button, true),
    a!("loop_move_right", "Move loop right", "Loop", Button, true),
    a!("beat_fx", "Beat FX on/off (slot 1)", "Effects", Button, true),
    a!("beat_fx_2", "Beat FX slot 2 on/off", "Effects", Button, true),
    a!("beat_fx_3", "Beat FX slot 3 on/off", "Effects", Button, true),
    a!("beat_fx_tap", "Beat FX tap", "Effects", Button, true),
    a!("fx_assign", "Assign Beat FX to this channel", "Effects", Button, true),
    a!("browse_forward", "Library: open / forward", "Library", Button, false),
    a!("browse_back", "Library: back", "Library", Button, false),
    a!("area", "Library: change panel", "Library", Button, false),
    a!("tag_track", "Add to Tag List", "Library", Button, false),
    a!("related_tracks", "Related tracks", "Library", Button, false),
    a!("fx_panel", "Show FX panel", "Library", Button, false),
    a!("sampler_panel", "Show sampler panel", "Library", Button, false),
    a!("master_cue", "Master cue", "Mixer", Button, false),
    a!("app.vinyl", "RekordJog: toggle vinyl (scratch) mode", "RekordJog", Button, false),
    a!("app.knob_mode", "RekordJog: switch Filter / Low EQ knobs", "RekordJog", Button, false),
    a!("app.fadefx", "RekordJog: toggle Fade FX", "RekordJog", Button, false),
    a!("app.pad_mode", "RekordJog: next pad mode", "RekordJog", Button, true),
    a!("none", "Do nothing", "General", Knob, false),
    a!("channel_fader", "Channel fader", "Mixer", Knob, true),
    a!("filter_or_low", "Filter (Color FX) / Low EQ – switchable", "Mixer", Knob, true),
    a!("cfx", "Filter (Color FX)", "Mixer", Knob, true),
    a!("eq_low", "EQ low", "Mixer", Knob, true),
    a!("eq_mid", "EQ mid", "Mixer", Knob, true),
    a!("eq_high", "EQ high", "Mixer", Knob, true),
    a!("trim", "Channel gain (trim)", "Mixer", Knob, true),
    a!("beat_fx_depth", "Beat FX depth", "Effects", Knob, true),
    a!("crossfader", "Crossfader", "Mixer", Knob, false),
    a!("crossfader_curve", "Crossfader curve", "Mixer", Knob, false),
    a!("sampler_volume", "Sampler volume", "Mixer", Knob, false),
    a!("none", "Do nothing", "General", Encoder, false),
    a!("browse", "Browse library", "Library", Encoder, false),
    a!("zoom", "Waveform zoom", "Library", Encoder, false),
    a!("fx1_beat", "Beat FX 1 beat length", "Effects", Encoder, false),
    a!("fx2_beat", "Beat FX 2 beat length", "Effects", Encoder, false),
];

/// Curated actions followed by every usable function from rekordbox's
/// own DDJ-SX mapping file.
pub fn actions() -> &'static [ActionInfo] {
    static ALL: std::sync::OnceLock<Vec<ActionInfo>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let mut v = CURATED.to_vec();
        v.extend(csv::table().iter().map(|c| c.info));
        v
    })
}

pub fn action_info(id: &str, kind: Kind) -> Option<&'static ActionInfo> {
    actions().iter().find(|a| a.id == id && a.kind == kind)
}

fn deck_note(d: u8, note: u8) -> Target {
    let status = 0x90 | d;
    Target::Note { status, note, led: Some((status, note)) }
}

fn global_note(note: u8, led: bool) -> Target {
    Target::Note {
        status: MIXER_STATUS_NOTE,
        note,
        led: if led { Some((MIXER_STATUS_NOTE, note)) } else { None },
    }
}

/// Resolve an action id into a concrete DDJ-SX target for a deck.
/// Returns `None` for unknown ids, or per-deck actions without a deck.
pub fn resolve(id: &str, side: Option<Side>) -> Option<Target> {
    let d = side.map(|s| s.idx() as u8);
    let deck = |f: &dyn Fn(u8) -> Target| d.map(f);
    if id.starts_with("ddj:") {
        return csv::resolve(id, side);
    }
    let t = match id {
        "none" => Target::None,
        "play" => return deck(&|d| deck_note(d, PLAY)),
        "play_shift" => return deck(&|d| deck_note(d, 0x47)),
        "cue" => return deck(&|d| deck_note(d, 0x0C)),
        "jump_start" => return deck(&|d| deck_note(d, 0x48)),
        "sync" => return deck(&|d| deck_note(d, 0x58)),
        "master" => return deck(&|d| deck_note(d, 0x5C)),
        "keylock" => return deck(&|d| deck_note(d, 0x1A)),
        "tempo_range" => return deck(&|d| deck_note(d, 0x60)),
        "slip" => return deck(&|d| deck_note(d, 0x40)),
        "slip_reverse" => return deck(&|d| deck_note(d, 0x15)),
        "reverse" => return deck(&|d| deck_note(d, 0x38)),
        "headphone_cue" => return deck(&|d| deck_note(d, 0x54)),
        "tap_bpm" => return deck(&|d| deck_note(d, 0x68)),
        "grid_adjust" => return deck(&|d| deck_note(d, 0x79)),
        "grid_slide" => return deck(&|d| deck_note(d, 0x0A)),
        "autoloop" => return deck(&|d| deck_note(d, 0x14)),
        "loop_active" => return deck(&|d| deck_note(d, 0x50)),
        "loop_half" => return deck(&|d| deck_note(d, 0x12)),
        "loop_double" => return deck(&|d| deck_note(d, 0x13)),
        "loop_in" => return deck(&|d| deck_note(d, 0x10)),
        "loop_out" => return deck(&|d| deck_note(d, 0x11)),
        "reloop_exit" => return deck(&|d| deck_note(d, 0x4D)),
        "loop_move_left" => return deck(&|d| deck_note(d, 0x61)),
        "loop_move_right" => return deck(&|d| deck_note(d, 0x62)),
        "load" => {
            return deck(&|d| Target::Note {
                status: MIXER_STATUS_NOTE,
                note: 0x46 + d,
                led: Some((LOADED_INDICATOR_STATUS, d)),
            })
        }
        // Left deck drives FX unit 1, right deck FX unit 2.
        "beat_fx" => return deck(&|d| fx_note(d, 0x47)),
        "beat_fx_2" => return deck(&|d| fx_note(d, 0x48)),
        "beat_fx_3" => return deck(&|d| fx_note(d, 0x49)),
        "beat_fx_tap" => return deck(&|d| fx_note(d, 0x4A)),
        "fx_assign" => return deck(&|d| global_note(if d == 0 { 0x4C } else { 0x51 }, true)),
        "browse_forward" => global_note(0x41, false),
        "browse_back" => global_note(0x65, false),
        "area" => global_note(0x66, false),
        "tag_track" => global_note(0x67, false),
        "related_tracks" => global_note(0x68, false),
        "fx_panel" => global_note(0x78, false),
        "sampler_panel" => global_note(0x79, false),
        "master_cue" => global_note(0x63, false),
        "app.vinyl" => Target::App(AppAction::ToggleVinyl),
        "app.knob_mode" => Target::App(AppAction::ToggleKnobMode),
        "app.fadefx" => Target::App(AppAction::ToggleFadeFx),
        "app.pad_mode" => return deck(&|_| Target::App(AppAction::NextPadMode)),
        "channel_fader" => return deck(&|d| Target::HiRes { status: 0xB0 | d, cc: 0x13 }),
        "filter_or_low" => return deck(&|_| Target::FilterOrLow),
        "cfx" => return deck(&|d| Target::HiRes { status: MIXER_STATUS_CC, cc: CFX_BASE + d }),
        "eq_low" => return deck(&|d| Target::HiRes { status: 0xB0 | d, cc: EQ_LOW }),
        "eq_mid" => return deck(&|d| Target::HiRes { status: 0xB0 | d, cc: 0x0B }),
        "eq_high" => return deck(&|d| Target::HiRes { status: 0xB0 | d, cc: 0x07 }),
        "trim" => return deck(&|d| Target::HiRes { status: 0xB0 | d, cc: 0x04 }),
        "beat_fx_depth" => return deck(&|d| Target::HiRes { status: 0xB4 + d, cc: 0x02 }),
        "crossfader" => Target::Crossfader,
        "crossfader_curve" => Target::HiRes { status: MIXER_STATUS_CC, cc: 0x01 },
        "sampler_volume" => Target::HiRes { status: MIXER_STATUS_CC, cc: 0x03 },
        "browse" => Target::Rotary { status: MIXER_STATUS_CC, cc: 0x40 },
        "zoom" => Target::Rotary { status: MIXER_STATUS_CC, cc: 0x64 },
        "fx1_beat" => Target::Rotary { status: 0xB4, cc: 0x00 },
        "fx2_beat" => Target::Rotary { status: 0xB5, cc: 0x00 },
        _ => return None,
    };
    Some(t)
}

fn fx_note(d: u8, note: u8) -> Target {
    let status = 0x94 + d;
    Target::Note { status, note, led: Some((status, note)) }
}

/// Convert a 7-bit controller value into the DDJ-SX 14-bit range so that
/// 0 → 0, 64 → exact centre (8192) and 127 → full scale (16383).
pub fn to_hires(v: u8) -> u16 {
    let v = v.min(127) as u32;
    if v <= 64 {
        (v * 128) as u16
    } else {
        (HIRES_CENTER as u32 + ((v - 64) * (HIRES_MAX - HIRES_CENTER) as u32 + 31) / 63) as u16
    }
}

pub fn split_hires(v: u16) -> (u8, u8) {
    let v = v.min(HIRES_MAX);
    ((v >> 7) as u8, (v & 0x7F) as u8)
}

/// rekordbox's stock `PIONEER DDJ-SX.midi.csv`, turned into mapping targets
/// so any control can trigger any function rekordbox supports on a DDJ-SX.
mod csv {
    use super::{ActionInfo, Target};
    use crate::{Kind, Side};
    use std::collections::HashSet;
    use std::sync::OnceLock;

    const SOURCE: &str = include_str!("../../docs/PIONEER DDJ-SX.midi.original.csv");

    #[derive(Clone, Copy, Debug)]
    enum Addr {
        /// Same message for every deck.
        Fixed(u8, u8),
        /// Status low nibble = channel listed per deck (deck 1, deck 2).
        Channel { status: u8, d1: u8, chans: [u8; 2] },
        /// Full message listed per deck.
        PerDeck([(u8, u8); 2]),
    }

    impl Addr {
        fn at(self, side: Option<Side>) -> Option<(u8, u8)> {
            match (self, side) {
                (Addr::Fixed(s, d), _) => Some((s, d)),
                (Addr::Channel { status, d1, chans }, Some(side)) => Some((status | chans[side.idx()], d1)),
                (Addr::PerDeck(v), Some(side)) => Some(v[side.idx()]),
                _ => None,
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct Row {
        pub info: ActionInfo,
        input: Addr,
        output: Option<Addr>,
        hires: bool,
    }

    fn hex2(s: &str) -> Option<(u8, u8)> {
        let s = s.trim();
        if s.len() != 4 {
            return None;
        }
        Some((u8::from_str_radix(&s[..2], 16).ok()?, u8::from_str_radix(&s[2..], 16).ok()?))
    }

    fn addr(code: &str, decks: &[&str]) -> Option<Addr> {
        let d1 = decks.first().map(|s| s.trim()).unwrap_or("");
        let d2 = decks.get(1).map(|s| s.trim()).unwrap_or("");
        match hex2(code) {
            Some((status, data)) if d1.is_empty() => Some(Addr::Fixed(status, data)),
            Some((status, data)) => {
                let c1: u8 = d1.parse().ok()?;
                let c2: u8 = d2.parse().ok()?;
                (c1 < 16 && c2 < 16).then_some(Addr::Channel { status: status & 0xF0, d1: data, chans: [c1, c2] })
            }
            None => Some(Addr::PerDeck([hex2(d1)?, hex2(d2)?])),
        }
    }

    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    pub fn table() -> &'static [Row] {
        static T: OnceLock<Vec<Row>> = OnceLock::new();
        T.get_or_init(|| {
            let mut rows = Vec::new();
            let mut seen = HashSet::new();
            let mut section = "Other";
            for line in SOURCE.lines() {
                let f: Vec<&'static str> = line.split(',').collect();
                if f.len() < 15 {
                    continue;
                }
                if let Some(sec) = f[0].strip_prefix("# ") {
                    section = sec.trim();
                    continue;
                }
                let (name, function, ty) = (f[0].trim(), f[1].trim(), f[2].trim());
                if name.is_empty() || name.starts_with('#') || name.starts_with('@') {
                    continue;
                }
                let (kind, hires) = match ty {
                    "Button" | "Pad" => (Kind::Button, false),
                    "KnobSliderHiRes" => (Kind::Knob, true),
                    "KnobSlider" => (Kind::Knob, false),
                    "Rotary" => (Kind::Encoder, false),
                    _ => continue,
                };
                if f[13].contains("RO") {
                    continue; // read-only state messages
                }
                let Some(input) = addr(f[3], &f[4..8]) else { continue };
                let output = if f[8].trim().is_empty() && f[9].trim().is_empty() { None } else { addr(f[8], &f[9..13]) };
                let key = if function.is_empty() { name } else { function };
                let mut id = format!("ddj:{key}");
                let mut n = 2;
                while !seen.insert(id.clone()) {
                    id = format!("ddj:{key}#{n}");
                    n += 1;
                }
                let comment = f[14].trim();
                let label = if comment.is_empty() { key.to_string() } else { format!("{key} — {comment}") };
                rows.push(Row {
                    info: ActionInfo {
                        id: leak(id),
                        label: leak(label),
                        group: leak(format!("rekordbox DDJ-SX: {section}")),
                        kind,
                        per_deck: !matches!(input, Addr::Fixed(..)),
                    },
                    input,
                    output,
                    hires,
                });
            }
            rows
        })
    }

    pub fn resolve(id: &str, side: Option<Side>) -> Option<Target> {
        let row = table().iter().find(|r| r.info.id == id)?;
        let (status, d1) = row.input.at(side)?;
        Some(match row.info.kind {
            Kind::Button => Target::Note { status, note: d1, led: row.output.and_then(|o| o.at(side)) },
            Kind::Knob if row.hires => Target::HiRes { status, cc: d1 },
            Kind::Knob => Target::Cc { status, cc: d1 },
            _ => Target::Rotary { status, cc: d1 },
        })
    }
}
