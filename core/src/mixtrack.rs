//! Numark Mixtrack Go MIDI implementation.
//!
//! Message numbers come from the community Mixxx mapping for this
//! controller. Left deck uses MIDI channel 1 (status nibble 0), right deck
//! channel 2 (nibble 1), pads use channels 5/6 and the mixer/browser
//! section uses channel 16. SHIFT (long-press MODE) is handled inside the
//! controller: it simply sends different note numbers while held.

use crate::{Kind, Side};

pub const LED_OFF: u8 = 0x00;
pub const LED_DIM: u8 = 0x01;
pub const LED_BRIGHT: u8 = 0x7F;

/// Universal identity request. Also stops the power-on light show.
pub const SYSEX_IDENTITY: [u8; 6] = [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7];
/// Asks the controller to report the position of every knob and fader.
pub const SYSEX_DUMP_STATE: [u8; 7] = [0xF0, 0x00, 0x20, 0x7F, 0x03, 0x01, 0xF7];

// Deck channel specials (status nibble = deck index).
pub const JOG_CC: u8 = 0x06;
pub const JOG_SHIFT_CC: u8 = 0x2B;
pub const TOUCH_NOTE: u8 = 0x50;
pub const TEMPO_MSB_CC: u8 = 0x09;
pub const TEMPO_LSB_CC: u8 = 0x29;
/// Sent while MODE is long-pressed (shift held).
pub const SHIFT_NOTE: u8 = 0x20;

// Pad channel specials (status nibble = 4 + deck index).
pub const PAD_CHANNEL_BASE: u8 = 4;
pub const MODE_NOTE: u8 = 0x00;
pub const PAD_NOTE_BASE: u8 = 0x14;
pub const PAD_SHIFT_NOTE_BASE: u8 = 0x1C;
/// Mode indicator LEDs on the pad channel: HOT CUE, LOOPS, SAMPLER, STEMS.
pub const MODE_LED_NOTES: [u8; 4] = [0x01, 0x02, 0x03, 0x04];

/// A remappable control of the Mixtrack Go.
#[derive(Clone, Debug)]
pub struct Control {
    pub id: String,
    pub label: String,
    pub section: String,
    pub kind: Kind,
    pub side: Option<Side>,
    /// Status byte of the message (0x9n for buttons, 0xBn for CCs).
    pub status: u8,
    pub data1: u8,
    /// LED address (status, note) if the control has a light.
    pub led: Option<(u8, u8)>,
    pub default_action: &'static str,
}

struct DeckDef {
    key: &'static str,
    label: &'static str,
    kind: Kind,
    /// status nibble added to the deck channel
    status: u8,
    data1: u8,
    led_status: Option<u8>,
    default_action: &'static str,
}

const DECK_CONTROLS: &[DeckDef] = &[
    DeckDef { key: "play", label: "Play / Pause", kind: Kind::Button, status: 0x90, data1: 0x00, led_status: Some(0x90), default_action: "play" },
    DeckDef { key: "play_shift", label: "Shift + Play", kind: Kind::Button, status: 0x90, data1: 0x04, led_status: Some(0x90), default_action: "play_shift" },
    DeckDef { key: "cue", label: "Cue", kind: Kind::Button, status: 0x90, data1: 0x01, led_status: Some(0x90), default_action: "cue" },
    DeckDef { key: "cue_shift", label: "Shift + Cue", kind: Kind::Button, status: 0x90, data1: 0x05, led_status: Some(0x90), default_action: "jump_start" },
    DeckDef { key: "sync", label: "Sync", kind: Kind::Button, status: 0x90, data1: 0x02, led_status: Some(0x90), default_action: "sync" },
    DeckDef { key: "sync_shift", label: "Shift + Sync", kind: Kind::Button, status: 0x90, data1: 0x03, led_status: Some(0x90), default_action: "master" },
    DeckDef { key: "acapel", label: "Acapel", kind: Kind::Button, status: 0x90, data1: 0x46, led_status: Some(0x90), default_action: "autoloop" },
    DeckDef { key: "acapel_shift", label: "Shift + Acapel", kind: Kind::Button, status: 0x90, data1: 0x47, led_status: Some(0x90), default_action: "loop_half" },
    DeckDef { key: "instru", label: "Instru", kind: Kind::Button, status: 0x90, data1: 0x48, led_status: Some(0x90), default_action: "beat_fx" },
    DeckDef { key: "instru_shift", label: "Shift + Instru", kind: Kind::Button, status: 0x90, data1: 0x49, led_status: Some(0x90), default_action: "loop_double" },
    // The Mixxx mapping drives the headphone LED with a note-off status byte.
    DeckDef { key: "pfl", label: "Headphone cue", kind: Kind::Button, status: 0x90, data1: 0x1B, led_status: Some(0x80), default_action: "headphone_cue" },
    DeckDef { key: "pfl_shift", label: "Shift + Headphone cue", kind: Kind::Button, status: 0x90, data1: 0x54, led_status: Some(0x90), default_action: "tap_bpm" },
    DeckDef { key: "level", label: "Channel level", kind: Kind::Knob, status: 0xB0, data1: 0x16, led_status: None, default_action: "channel_fader" },
    DeckDef { key: "filter", label: "Filter / Low knob", kind: Kind::Knob, status: 0xB0, data1: 0x1A, led_status: None, default_action: "filter_or_low" },
];

/// All remappable controls with both decks expanded.
pub fn controls() -> Vec<Control> {
    let mut out = Vec::new();
    for side in Side::BOTH {
        let ch = side.idx() as u8;
        for d in DECK_CONTROLS {
            out.push(Control {
                id: format!("{}.{}", side.key(), d.key),
                label: d.label.to_string(),
                section: side.title().to_string(),
                kind: d.kind,
                side: Some(side),
                status: d.status | ch,
                data1: d.data1,
                led: d.led_status.map(|s| (s | ch, d.data1)),
                default_action: d.default_action,
            });
        }
    }

    let mixer = |id: &str, label: &str, kind, side, status, data1, led: bool, def: &'static str| Control {
        id: id.to_string(),
        label: label.to_string(),
        section: "Mixer & browser".to_string(),
        kind,
        side,
        status,
        data1,
        led: if led { Some((status, data1)) } else { None },
        default_action: def,
    };
    out.push(mixer("crossfader", "Crossfader", Kind::Knob, None, 0xBF, 0x08, false, "crossfader"));
    out.push(mixer("main_level", "Main level knob", Kind::Knob, None, 0xBF, 0x0A, false, "none"));
    out.push(mixer("cue_level", "Cue level knob", Kind::Knob, None, 0xBF, 0x0C, false, "none"));
    out.push(mixer("browse", "Browse knob (turn)", Kind::Encoder, None, 0xBF, 0x00, false, "browse"));
    out.push(mixer("browse_shift", "Shift + Browse knob (turn)", Kind::Encoder, None, 0xBF, 0x01, false, "zoom"));
    out.push(mixer("browse_press", "Browse knob (push)", Kind::Button, None, 0x9F, 0x07, false, "browse_forward"));
    out.push(mixer("load_left", "Load left", Kind::Button, Some(Side::Left), 0x9F, 0x02, true, "load"));
    out.push(mixer("load_right", "Load right", Kind::Button, Some(Side::Right), 0x9F, 0x03, true, "load"));
    out.push(mixer("load_left_shift", "Shift + Load left", Kind::Button, Some(Side::Left), 0x9F, 0x48, true, "app.knob_mode"));
    out.push(mixer("load_right_shift", "Shift + Load right", Kind::Button, Some(Side::Right), 0x9F, 0x49, true, "app.vinyl"));
    out.push(mixer("fadefx", "Fade FX", Kind::Button, None, 0x9F, 0x46, true, "app.fadefx"));
    out.push(mixer("fadefx_shift", "Shift + Fade FX", Kind::Button, None, 0x9F, 0x47, true, "browse_back"));
    out
}

/// LED address of pad `i` (0..4) for a deck.
pub fn pad_led(side: Side, i: usize, shifted: bool) -> (u8, u8) {
    let base = if shifted { PAD_SHIFT_NOTE_BASE } else { PAD_NOTE_BASE };
    (0x90 | (PAD_CHANNEL_BASE + side.idx() as u8), base + i as u8)
}

/// LED address of mode indicator `i` (0..4) for a deck.
pub fn mode_led(side: Side, i: usize) -> (u8, u8) {
    (0x90 | (PAD_CHANNEL_BASE + side.idx() as u8), MODE_LED_NOTES[i])
}
