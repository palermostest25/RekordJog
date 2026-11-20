use crate::config::Config;
use crate::ddj::{self, AppAction, Target, PAD_MODES};
use crate::mixtrack::{self as mt, Control, LED_BRIGHT, LED_DIM, LED_OFF};
use crate::{Kind, Side};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};

/// Mixtrack Go jog wheels report ~242 ticks per revolution; rekordbox's
/// DDJ-SX profile expects 2048. This keeps the platter 1:1 with the
/// on-screen record at 100 % sensitivity.
pub const MIXTRACK_JOG_TICKS: f64 = 242.0;
pub const DDJ_JOG_TICKS: f64 = 2048.0;
const JOG_BASE: f64 = DDJ_JOG_TICKS / MIXTRACK_JOG_TICKS;

const LED_REFRESH_DELAY_MS: u64 = 1500;
const BLINK_MS: u64 = 500;
const MAX_EVENTS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Dest {
    /// The virtual DDJ-SX port (read by rekordbox).
    Rekordbox,
    /// The physical Mixtrack Go (LEDs, init sysex).
    Controller,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Out {
    pub dest: Dest,
    pub bytes: Vec<u8>,
}

impl Out {
    fn rb(bytes: &[u8]) -> Out {
        Out { dest: Dest::Rekordbox, bytes: bytes.to_vec() }
    }
    fn ctl(bytes: &[u8]) -> Out {
        Out { dest: Dest::Controller, bytes: bytes.to_vec() }
    }
}

/// A control was used; drives the live controller view and setup tests.
#[derive(Clone, Debug, Serialize)]
pub struct UiEvent {
    pub id: String,
    pub value: u8,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeckSnapshot {
    pub pad_mode: &'static str,
    pub touched: bool,
    pub playing: bool,
    /// Tempo fader position -1.0 (top) .. 1.0 (bottom), if moved yet.
    pub tempo: Option<f32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub decks: [DeckSnapshot; 2],
    pub vinyl: bool,
    pub knob_mode: &'static str,
    pub fadefx: bool,
    pub controller_messages: u64,
    pub rekordbox_messages: u64,
}

#[derive(Clone, Debug)]
struct Deck {
    touched: bool,
    touch_sent: bool,
    jog_cc: u8,
    accum: f64,
    tempo_msb: u8,
    tempo_lsb: u8,
    tempo_seen: bool,
    pad_mode: usize,
    /// pad note pressed per pad (to release the same note)
    pad_down: [Option<u8>; 4],
    /// last LED value rekordbox sent per DDJ-SX pad note
    pad_leds: [u8; 128],
    playing: bool,
}

impl Default for Deck {
    fn default() -> Self {
        Deck {
            touched: false,
            touch_sent: false,
            jog_cc: 0,
            accum: 0.0,
            tempo_msb: 64,
            tempo_lsb: 0,
            tempo_seen: false,
            pad_mode: 0,
            pad_down: [None; 4],
            pad_leds: [0; 128],
            playing: false,
        }
    }
}

pub struct Engine {
    cfg: Config,
    controls: Vec<Control>,
    targets: Vec<Target>,
    input_map: HashMap<(u8, u8), usize>,
    /// rekordbox LED address -> controls that show it
    led_map: HashMap<(u8, u8), Vec<usize>>,
    /// last LED state rekordbox reported, per control
    led_on: Vec<bool>,
    /// last raw value per control (knobs) for re-sync
    last_value: Vec<Option<u8>>,
    decks: [Deck; 2],
    vinyl: bool,
    knob_filter: bool,
    fadefx: bool,
    blink_on: bool,
    next_blink: u64,
    refresh_at: Option<u64>,
    led_cache: HashMap<(u8, u8), u8>,
    events: VecDeque<UiEvent>,
    controller_messages: u64,
    rekordbox_messages: u64,
}

fn norm(status: u8) -> u8 {
    // treat note-off as note-on so lookups are uniform
    if status & 0xF0 == 0x80 {
        0x90 | (status & 0x0F)
    } else {
        status
    }
}

impl Engine {
    pub fn new(cfg: Config) -> Engine {
        let cfg = cfg.sanitized();
        let controls = mt::controls();
        let n = controls.len();
        let mut e = Engine {
            vinyl: cfg.vinyl_default,
            knob_filter: cfg.knob_default_filter,
            cfg,
            controls,
            targets: vec![Target::None; n],
            input_map: HashMap::new(),
            led_map: HashMap::new(),
            led_on: vec![false; n],
            last_value: vec![None; n],
            decks: [Deck::default(), Deck::default()],
            fadefx: false,
            blink_on: false,
            next_blink: 0,
            refresh_at: None,
            led_cache: HashMap::new(),
            events: VecDeque::new(),
            controller_messages: 0,
            rekordbox_messages: 0,
        };
        e.rebuild();
        e
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Apply new settings without losing runtime state. Returns LED updates.
    pub fn set_config(&mut self, cfg: Config) -> Vec<Out> {
        self.cfg = cfg.sanitized();
        self.rebuild();
        self.refresh_leds()
    }

    /// Action id currently bound to a control.
    pub fn binding(&self, control_id: &str) -> Option<String> {
        let c = self.controls.iter().find(|c| c.id == control_id)?;
        Some(self.binding_for(c))
    }

    fn binding_for(&self, c: &Control) -> String {
        match self.cfg.bindings.get(&c.id) {
            Some(id) if Self::valid_binding(c, id) => id.clone(),
            _ => c.default_action.to_string(),
        }
    }

    fn valid_binding(c: &Control, id: &str) -> bool {
        match ddj::action_info(id, c.kind) {
            Some(info) => !(info.per_deck && c.side.is_none()),
            None => false,
        }
    }

    fn rebuild(&mut self) {
        self.input_map.clear();
        self.led_map.clear();
        for i in 0..self.controls.len() {
            let c = &self.controls[i];
            let action = self.binding_for(c);
            let target = ddj::resolve(&action, c.side).unwrap_or(Target::None);
            self.input_map.insert((c.status, c.data1), i);
            if let Target::Note { led: Some(addr), .. } = target {
                self.led_map.entry(addr).or_default().push(i);
            }
            self.targets[i] = target;
        }
    }

    // ------------------------------------------------------------------
    // Lifecycle
    // ------------------------------------------------------------------

    /// Called when the Mixtrack Go has been (re)opened.
    pub fn controller_connected(&mut self, now_ms: u64) -> Vec<Out> {
        let mut out = vec![Out::ctl(&mt::SYSEX_IDENTITY), Out::ctl(&mt::SYSEX_DUMP_STATE)];
        out.extend(self.refresh_leds());
        // The power-on light show can overwrite LEDs, so paint them again.
        self.refresh_at = Some(now_ms + LED_REFRESH_DELAY_MS);
        out
    }

    /// Called when the controller disappears: release anything held.
    pub fn controller_disconnected(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        for d in 0..2 {
            let deck = &mut self.decks[d];
            if deck.touch_sent {
                out.push(Out::rb(&[0x90 | d as u8, ddj::JOG_TOUCH, 0]));
            }
            deck.touched = false;
            deck.touch_sent = false;
            for p in 0..4 {
                if let Some(note) = deck.pad_down[p].take() {
                    out.push(Out::rb(&[0x90 | (ddj::PAD_CHANNEL_BASE + d as u8), note, 0]));
                }
            }
        }
        self.led_cache.clear();
        out
    }

    /// Re-send the full controller state to rekordbox (after rekordbox
    /// starts or the virtual port is recreated).
    pub fn rekordbox_resync(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        for d in 0..2u8 {
            out.push(Out::rb(&[0x90 | d, ddj::VINYL_STATE, if self.vinyl { 0x7F } else { 0 }]));
            let mode = PAD_MODES[self.decks[d as usize].pad_mode];
            out.push(Out::rb(&[0x90 | d, mode.mode_note, 0x7F]));
            out.push(Out::rb(&[0x90 | d, mode.mode_note, 0x00]));
            if self.decks[d as usize].tempo_seen {
                self.send_tempo(d as usize, &mut out);
            }
        }
        for i in 0..self.controls.len() {
            if let Some(v) = self.last_value[i] {
                if self.controls[i].kind == Kind::Knob {
                    self.apply_knob(i, v, &mut out);
                }
            }
        }
        out
    }

    /// Periodic housekeeping (call every ~20-50 ms).
    pub fn tick(&mut self, now_ms: u64) -> Vec<Out> {
        let mut out = Vec::new();
        if let Some(at) = self.refresh_at {
            if now_ms >= at {
                self.refresh_at = None;
                self.led_cache.clear();
                out.extend(self.refresh_leds());
            }
        }
        if now_ms >= self.next_blink {
            self.next_blink = now_ms + BLINK_MS;
            self.blink_on = !self.blink_on;
            if self.fadefx {
                self.paint_app_leds(&mut out);
            }
        }
        out
    }

    pub fn drain_events(&mut self) -> Vec<UiEvent> {
        self.events.drain(..).collect()
    }

    pub fn snapshot(&self) -> Snapshot {
        let deck = |d: usize| {
            let k = &self.decks[d];
            DeckSnapshot {
                pad_mode: PAD_MODES[k.pad_mode].name,
                touched: k.touched,
                playing: k.playing,
                tempo: if k.tempo_seen {
                    let v = ((k.tempo_msb as u16) << 7 | k.tempo_lsb as u16) as f32;
                    Some((v - 8192.0) / 8191.5)
                } else {
                    None
                },
            }
        };
        Snapshot {
            decks: [deck(0), deck(1)],
            vinyl: self.vinyl,
            knob_mode: if self.knob_filter { "filter" } else { "eq_low" },
            fadefx: self.fadefx,
            controller_messages: self.controller_messages,
            rekordbox_messages: self.rekordbox_messages,
        }
    }

    fn event(&mut self, id: String, value: u8) {
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(UiEvent { id, value });
    }

    // ------------------------------------------------------------------
    // Controller -> rekordbox
    // ------------------------------------------------------------------

    pub fn on_controller(&mut self, bytes: &[u8]) -> Vec<Out> {
        let mut out = Vec::new();
        if bytes.len() < 3 || bytes[0] < 0x80 || bytes[0] >= 0xF0 {
            return out;
        }
        self.controller_messages += 1;
        let (status, d1, d2) = (bytes[0], bytes[1], bytes[2]);
        let kind = status & 0xF0;
        let ch = status & 0x0F;
        let is_note = kind == 0x80 || kind == 0x90;
        let pressed = kind == 0x90 && d2 > 0;
        let value = if kind == 0x80 { 0 } else { d2 };

        // Built-in deck behaviour
        if ch < 2 {
            let d = ch as usize;
            let side = Side::from_idx(d);
            if kind == 0xB0 {
                match d1 {
                    mt::JOG_CC | mt::JOG_SHIFT_CC => {
                        self.jog(d, d2, d1 == mt::JOG_SHIFT_CC, &mut out);
                        self.event(format!("{}.jog", side.key()), d2);
                        return out;
                    }
                    mt::TEMPO_MSB_CC | mt::TEMPO_LSB_CC => {
                        let deck = &mut self.decks[d];
                        if d1 == mt::TEMPO_MSB_CC {
                            deck.tempo_msb = d2 & 0x7F;
                        } else {
                            deck.tempo_lsb = d2 & 0x7F;
                        }
                        deck.tempo_seen = true;
                        self.send_tempo(d, &mut out);
                        self.event(format!("{}.tempo", side.key()), self.decks[d].tempo_msb);
                        return out;
                    }
                    _ => {}
                }
            } else if is_note {
                match d1 {
                    mt::TOUCH_NOTE => {
                        self.touch(d, pressed, &mut out);
                        self.event(format!("{}.touch", side.key()), value);
                        return out;
                    }
                    mt::SHIFT_NOTE => {
                        out.push(Out::rb(&[0x90 | ch, ddj::SHIFT, if pressed { 0x7F } else { 0 }]));
                        self.event(format!("{}.shift", side.key()), value);
                        return out;
                    }
                    _ => {}
                }
            }
        } else if is_note && (ch == mt::PAD_CHANNEL_BASE || ch == mt::PAD_CHANNEL_BASE + 1) {
            let d = (ch - mt::PAD_CHANNEL_BASE) as usize;
            let side = Side::from_idx(d);
            if d1 == mt::MODE_NOTE {
                if pressed {
                    self.next_pad_mode(d, &mut out);
                }
                self.event(format!("{}.mode", side.key()), value);
                return out;
            }
            let pad = if (mt::PAD_NOTE_BASE..mt::PAD_NOTE_BASE + 4).contains(&d1) {
                Some(((d1 - mt::PAD_NOTE_BASE) as usize, false))
            } else if (mt::PAD_SHIFT_NOTE_BASE..mt::PAD_SHIFT_NOTE_BASE + 4).contains(&d1) {
                Some(((d1 - mt::PAD_SHIFT_NOTE_BASE) as usize, true))
            } else {
                None
            };
            if let Some((p, shifted)) = pad {
                self.pad(d, p, shifted, pressed, &mut out);
                self.event(format!("{}.pad{}{}", side.key(), p + 1, if shifted { "_shift" } else { "" }), value);
                return out;
            }
        }

        let key = (if is_note { norm(status) } else { status }, d1);
        if let Some(&i) = self.input_map.get(&key) {
            self.event(self.controls[i].id.clone(), value);
            match self.controls[i].kind {
                Kind::Button => self.apply_button(i, pressed, &mut out),
                Kind::Knob => {
                    self.last_value[i] = Some(value);
                    self.apply_knob(i, value, &mut out);
                }
                Kind::Encoder => self.apply_encoder(i, value, &mut out),
                Kind::Fixed => {}
            }
        }
        out
    }

    fn apply_button(&mut self, i: usize, pressed: bool, out: &mut Vec<Out>) {
        match self.targets[i] {
            Target::Note { status, note, led } => {
                out.push(Out::rb(&[status, note, if pressed { 0x7F } else { 0x00 }]));
                if led.is_none() {
                    // no feedback from rekordbox: light while held
                    if let Some(addr) = self.controls[i].led {
                        self.led(addr, if pressed { LED_BRIGHT } else { LED_DIM }, out);
                    }
                }
            }
            Target::App(a) => {
                if pressed {
                    self.app_action(a, self.controls[i].side, out);
                }
            }
            _ => {}
        }
    }

    fn apply_knob(&mut self, i: usize, v: u8, out: &mut Vec<Out>) {
        match self.targets[i] {
            Target::HiRes { status, cc } => hires(status, cc, ddj::to_hires(v), out),
            Target::Crossfader => {
                hires(ddj::MIXER_STATUS_CC, ddj::CROSSFADER, ddj::to_hires(v), out);
                if self.fadefx {
                    self.fade_fx(v, out);
                }
            }
            Target::FilterOrLow => {
                if let Some(side) = self.controls[i].side {
                    let (status, cc) = self.filter_target(side.idx() as u8);
                    hires(status, cc, ddj::to_hires(v), out);
                }
            }
            _ => {}
        }
    }

    fn apply_encoder(&mut self, i: usize, v: u8, out: &mut Vec<Out>) {
        if let Target::Rotary { status, cc } = self.targets[i] {
            let v = if self.cfg.invert_browse && v != 0 { 128 - v as u16 } else { v as u16 } as u8;
            out.push(Out::rb(&[status, cc, v & 0x7F]));
        }
    }

    fn filter_target(&self, d: u8) -> (u8, u8) {
        if self.knob_filter {
            (ddj::MIXER_STATUS_CC, ddj::CFX_BASE + d)
        } else {
            (0xB0 | d, ddj::EQ_LOW)
        }
    }

    fn send_tempo(&self, d: usize, out: &mut Vec<Out>) {
        let deck = &self.decks[d];
        let mut v = ((deck.tempo_msb as u16) << 7) | deck.tempo_lsb as u16;
        if self.cfg.invert_tempo {
            v = ddj::HIRES_MAX - v;
        }
        let (msb, lsb) = ddj::split_hires(v);
        let st = 0xB0 | d as u8;
        out.push(Out::rb(&[st, ddj::TEMPO_MSB, msb]));
        out.push(Out::rb(&[st, ddj::TEMPO_LSB, lsb]));
    }

    fn touch(&mut self, d: usize, pressed: bool, out: &mut Vec<Out>) {
        let vinyl = self.vinyl;
        let deck = &mut self.decks[d];
        deck.touched = pressed;
        deck.accum = 0.0;
        if pressed && vinyl {
            out.push(Out::rb(&[0x90 | d as u8, ddj::JOG_TOUCH, 0x7F]));
            deck.touch_sent = true;
        } else if !pressed && deck.touch_sent {
            out.push(Out::rb(&[0x90 | d as u8, ddj::JOG_TOUCH, 0x00]));
            deck.touch_sent = false;
        }
    }

    fn jog(&mut self, d: usize, raw: u8, shifted: bool, out: &mut Vec<Out>) {
        let delta = if raw < 64 { raw as i32 } else { raw as i32 - 128 };
        if delta == 0 {
            return;
        }
        let vinyl = self.vinyl;
        let (scratch, bend) = (self.cfg.scratch_sensitivity, self.cfg.bend_sensitivity);
        let deck = &mut self.decks[d];
        let (cc, sens) = if shifted {
            (if deck.touched { ddj::JOG_SEARCH } else { ddj::WHEEL_SEARCH }, bend)
        } else if deck.touched && vinyl {
            (ddj::JOG_SCRATCH, scratch)
        } else if deck.touched {
            (ddj::JOG_PITCH_BEND, bend)
        } else {
            (ddj::WHEEL_PITCH_BEND, bend)
        };
        if deck.jog_cc != cc {
            deck.jog_cc = cc;
            deck.accum = 0.0;
        }
        deck.accum += delta as f64 * JOG_BASE * sens;
        let mut n = deck.accum.trunc() as i32;
        deck.accum -= n as f64;
        while n != 0 {
            let step = n.clamp(-63, 63);
            out.push(Out::rb(&[0xB0 | d as u8, cc, (64 + step) as u8]));
            n -= step;
        }
    }

    fn pad(&mut self, d: usize, p: usize, shifted: bool, pressed: bool, out: &mut Vec<Out>) {
        let status = 0x90 | (ddj::PAD_CHANNEL_BASE + d as u8);
        let deck = &mut self.decks[d];
        if pressed {
            let note = PAD_MODES[deck.pad_mode].base + p as u8 + if shifted { 8 } else { 0 };
            if let Some(prev) = deck.pad_down[p].replace(note) {
                if prev != note {
                    out.push(Out::rb(&[status, prev, 0]));
                }
            }
            out.push(Out::rb(&[status, note, 0x7F]));
        } else if let Some(note) = deck.pad_down[p].take() {
            out.push(Out::rb(&[status, note, 0x00]));
        }
    }

    fn next_pad_mode(&mut self, d: usize, out: &mut Vec<Out>) {
        // release held pads so nothing gets stuck in the old mode
        let status = 0x90 | (ddj::PAD_CHANNEL_BASE + d as u8);
        for p in 0..4 {
            if let Some(note) = self.decks[d].pad_down[p].take() {
                out.push(Out::rb(&[status, note, 0]));
            }
        }
        let deck = &mut self.decks[d];
        deck.pad_mode = (deck.pad_mode + 1) % PAD_MODES.len();
        let mode = PAD_MODES[deck.pad_mode];
        out.push(Out::rb(&[0x90 | d as u8, mode.mode_note, 0x7F]));
        out.push(Out::rb(&[0x90 | d as u8, mode.mode_note, 0x00]));
        self.paint_pads(d, out);
    }

    fn app_action(&mut self, a: AppAction, side: Option<Side>, out: &mut Vec<Out>) {
        match a {
            AppAction::ToggleVinyl => {
                self.vinyl = !self.vinyl;
                for d in 0..2u8 {
                    out.push(Out::rb(&[0x90 | d, ddj::VINYL_STATE, if self.vinyl { 0x7F } else { 0 }]));
                    let deck = &mut self.decks[d as usize];
                    if !self.vinyl && deck.touch_sent {
                        out.push(Out::rb(&[0x90 | d, ddj::JOG_TOUCH, 0]));
                        deck.touch_sent = false;
                    }
                }
            }
            AppAction::ToggleKnobMode => {
                // park the old destination in neutral, the knob takes over on its next move
                for d in 0..2u8 {
                    let (status, cc) = self.filter_target(d);
                    hires(status, cc, ddj::HIRES_CENTER, out);
                }
                self.knob_filter = !self.knob_filter;
            }
            AppAction::ToggleFadeFx => {
                self.fadefx = !self.fadefx;
                if self.fadefx {
                    if let Some(v) = self.crossfader_value() {
                        self.fade_fx(v, out);
                    }
                } else {
                    self.restore_cfx(out);
                }
            }
            AppAction::NextPadMode => {
                if let Some(s) = side {
                    self.next_pad_mode(s.idx(), out);
                }
            }
        }
        self.paint_app_leds(out);
    }

    fn crossfader_value(&self) -> Option<u8> {
        (0..self.controls.len())
            .find(|&i| self.targets[i] == Target::Crossfader)
            .and_then(|i| self.last_value[i])
    }

    /// Fade FX: moving the crossfader away from a deck sweeps that deck's
    /// Color FX filter open, washing it out as it fades.
    fn fade_fx(&self, xf: u8, out: &mut Vec<Out>) {
        let x = xf as f64 / 127.0;
        let span = (ddj::HIRES_MAX - ddj::HIRES_CENTER) as f64 * self.cfg.fadefx_depth;
        let amounts = [((x - 0.5) * 2.0).max(0.0), ((0.5 - x) * 2.0).max(0.0)];
        for (d, amt) in amounts.iter().enumerate() {
            let v = ddj::HIRES_CENTER as f64 + amt * span;
            hires(ddj::MIXER_STATUS_CC, ddj::CFX_BASE + d as u8, v.round() as u16, out);
        }
    }

    fn restore_cfx(&self, out: &mut Vec<Out>) {
        for d in 0..2u8 {
            let mut v = ddj::HIRES_CENTER;
            if self.knob_filter {
                let side = Side::from_idx(d as usize);
                if let Some(i) = (0..self.controls.len())
                    .find(|&i| self.targets[i] == Target::FilterOrLow && self.controls[i].side == Some(side))
                {
                    if let Some(raw) = self.last_value[i] {
                        v = ddj::to_hires(raw);
                    }
                }
            }
            hires(ddj::MIXER_STATUS_CC, ddj::CFX_BASE + d, v, out);
        }
    }

    // ------------------------------------------------------------------
    // rekordbox -> controller (LEDs)
    // ------------------------------------------------------------------

    pub fn on_rekordbox(&mut self, bytes: &[u8]) -> Vec<Out> {
        let mut out = Vec::new();
        if bytes.len() < 3 || bytes[0] < 0x80 || bytes[0] >= 0xF0 {
            return out;
        }
        self.rekordbox_messages += 1;
        let (status, d1, d2) = (bytes[0], bytes[1], bytes[2]);
        let kind = status & 0xF0;
        let ch = status & 0x0F;
        if kind != 0x80 && kind != 0x90 {
            return out;
        }
        let value = if kind == 0x80 { 0 } else { d2 };
        let on = value > 0;

        if ch < 2 && d1 == ddj::PLAY {
            self.decks[ch as usize].playing = on;
        }
        if ch == ddj::PAD_CHANNEL_BASE || ch == ddj::PAD_CHANNEL_BASE + 1 {
            let d = (ch - ddj::PAD_CHANNEL_BASE) as usize;
            self.decks[d].pad_leds[d1 as usize & 0x7F] = value;
            let base = PAD_MODES[self.decks[d].pad_mode].base;
            if d1 >= base && d1 < base + 16 {
                let offset = d1 - base;
                let (p, shifted) = ((offset % 8) as usize, offset >= 8);
                if p < 4 {
                    let addr = mt::pad_led(Side::from_idx(d), p, shifted);
                    self.led(addr, if on { LED_BRIGHT } else { LED_OFF }, &mut out);
                }
            }
            return out;
        }

        let indices = match self.led_map.get(&(0x90 | ch, d1)) {
            Some(v) => v.clone(),
            None => return out,
        };
        for i in indices {
            self.led_on[i] = on;
            if let Some(addr) = self.controls[i].led {
                self.led(addr, if on { LED_BRIGHT } else { LED_DIM }, &mut out);
            }
        }
        out
    }

    fn led(&mut self, addr: (u8, u8), value: u8, out: &mut Vec<Out>) {
        if self.led_cache.get(&addr) == Some(&value) {
            return;
        }
        self.led_cache.insert(addr, value);
        out.push(Out::ctl(&[addr.0, addr.1, value]));
    }

    fn app_led_value(&self, a: AppAction) -> u8 {
        let on = match a {
            AppAction::ToggleVinyl => self.vinyl,
            AppAction::ToggleKnobMode => self.knob_filter,
            AppAction::ToggleFadeFx => self.fadefx && self.blink_on,
            AppAction::NextPadMode => false,
        };
        if on {
            LED_BRIGHT
        } else {
            LED_DIM
        }
    }

    fn paint_app_leds(&mut self, out: &mut Vec<Out>) {
        for i in 0..self.controls.len() {
            if let (Target::App(a), Some(addr)) = (self.targets[i], self.controls[i].led) {
                let v = self.app_led_value(a);
                self.led(addr, v, out);
            }
        }
    }

    fn paint_pads(&mut self, d: usize, out: &mut Vec<Out>) {
        let side = Side::from_idx(d);
        let mode = PAD_MODES[self.decks[d].pad_mode];
        for m in 0..4 {
            let v = if mode.leds.contains(&m) { LED_BRIGHT } else { LED_DIM };
            self.led(mt::mode_led(side, m), v, out);
        }
        for p in 0..4 {
            for shifted in [false, true] {
                let note = mode.base + p as u8 + if shifted { 8 } else { 0 };
                let on = self.decks[d].pad_leds[note as usize] > 0;
                self.led(mt::pad_led(side, p, shifted), if on { LED_BRIGHT } else { LED_OFF }, out);
            }
        }
    }

    /// Paint every LED from the current state.
    pub fn refresh_leds(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        for i in 0..self.controls.len() {
            let Some(addr) = self.controls[i].led else { continue };
            let v = match self.targets[i] {
                Target::App(a) => self.app_led_value(a),
                Target::Note { led: Some(_), .. } if self.led_on[i] => LED_BRIGHT,
                _ => LED_DIM,
            };
            self.led(addr, v, &mut out);
        }
        for d in 0..2 {
            self.paint_pads(d, &mut out);
        }
        out
    }
}

fn hires(status: u8, cc: u8, v: u16, out: &mut Vec<Out>) {
    let (msb, lsb) = ddj::split_hires(v);
    out.push(Out::rb(&[status, cc, msb]));
    out.push(Out::rb(&[status, cc + 0x20, lsb]));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rb(out: &[Out]) -> Vec<Vec<u8>> {
        out.iter().filter(|o| o.dest == Dest::Rekordbox).map(|o| o.bytes.clone()).collect()
    }
    fn ctl(out: &[Out]) -> Vec<Vec<u8>> {
        out.iter().filter(|o| o.dest == Dest::Controller).map(|o| o.bytes.clone()).collect()
    }
    fn engine() -> Engine {
        let mut e = Engine::new(Config::default());
        e.controller_connected(0);
        e
    }

    #[test]
    fn hires_scaling() {
        assert_eq!(ddj::to_hires(0), 0);
        assert_eq!(ddj::to_hires(64), 8192);
        assert_eq!(ddj::to_hires(127), 16383);
        assert!(ddj::to_hires(100) > ddj::to_hires(99));
        assert_eq!(ddj::split_hires(16383), (127, 127));
    }

    #[test]
    fn play_button_and_led() {
        let mut e = engine();
        assert_eq!(rb(&e.on_controller(&[0x90, 0x00, 0x7F])), vec![vec![0x90, 0x0B, 0x7F]]);
        assert_eq!(rb(&e.on_controller(&[0x80, 0x00, 0x00])), vec![vec![0x90, 0x0B, 0x00]]);
        // right deck
        assert_eq!(rb(&e.on_controller(&[0x91, 0x00, 0x7F])), vec![vec![0x91, 0x0B, 0x7F]]);
        // LED from rekordbox
        assert_eq!(ctl(&e.on_rekordbox(&[0x91, 0x0B, 0x7F])), vec![vec![0x91, 0x00, 0x7F]]);
        assert!(e.snapshot().decks[1].playing);
        assert_eq!(ctl(&e.on_rekordbox(&[0x91, 0x0B, 0x00])), vec![vec![0x91, 0x00, LED_DIM]]);
        // duplicate LED states are not re-sent
        assert!(ctl(&e.on_rekordbox(&[0x91, 0x0B, 0x00])).is_empty());
    }

    #[test]
    fn shifted_buttons() {
        let mut e = engine();
        assert_eq!(rb(&e.on_controller(&[0x90, 0x05, 0x7F])), vec![vec![0x90, 0x48, 0x7F]]);
        assert_eq!(rb(&e.on_controller(&[0x91, 0x03, 0x7F])), vec![vec![0x91, 0x5C, 0x7F]]);
    }

    #[test]
    fn jog_untouched_is_pitch_bend() {
        let mut e = engine();
        let out = rb(&e.on_controller(&[0xB0, 0x06, 0x01]));
        assert_eq!(out, vec![vec![0xB0, ddj::WHEEL_PITCH_BEND, 64 + 8]]);
        let out = rb(&e.on_controller(&[0xB0, 0x06, 0x7F]));
        assert_eq!(out[0][1], ddj::WHEEL_PITCH_BEND);
        assert!(out[0][2] < 64);
    }

    #[test]
    fn jog_accumulates_fractional_ticks() {
        let mut e = engine();
        let mut total = 0i32;
        for _ in 0..242 {
            for m in rb(&e.on_controller(&[0xB0, 0x06, 0x01])) {
                total += m[2] as i32 - 64;
            }
        }
        assert!((total - 2048).abs() <= 1, "one revolution = {total}");
    }

    #[test]
    fn touch_scratch_in_vinyl_mode() {
        let mut e = engine();
        assert_eq!(rb(&e.on_controller(&[0x91, 0x50, 0x7F])), vec![vec![0x91, ddj::JOG_TOUCH, 0x7F]]);
        assert_eq!(rb(&e.on_controller(&[0xB1, 0x06, 0x01]))[0][1], ddj::JOG_SCRATCH);
        assert_eq!(rb(&e.on_controller(&[0x81, 0x50, 0x00])), vec![vec![0x91, ddj::JOG_TOUCH, 0x00]]);
        assert_eq!(rb(&e.on_controller(&[0xB1, 0x06, 0x01]))[0][1], ddj::WHEEL_PITCH_BEND);
    }

    #[test]
    fn touch_without_vinyl_bends() {
        let mut e = Engine::new(Config { vinyl_default: false, ..Config::default() });
        assert!(rb(&e.on_controller(&[0x90, 0x50, 0x7F])).is_empty());
        assert_eq!(rb(&e.on_controller(&[0xB0, 0x06, 0x01]))[0][1], ddj::JOG_PITCH_BEND);
    }

    #[test]
    fn shift_jog_searches() {
        let mut e = engine();
        assert_eq!(rb(&e.on_controller(&[0xB0, 0x2B, 0x01]))[0][1], ddj::WHEEL_SEARCH);
        e.on_controller(&[0x90, 0x50, 0x7F]);
        assert_eq!(rb(&e.on_controller(&[0xB0, 0x2B, 0x01]))[0][1], ddj::JOG_SEARCH);
    }

    #[test]
    fn vinyl_toggle_from_shift_load() {
        let mut e = engine();
        e.on_controller(&[0x90, 0x50, 0x7F]);
        let out = e.on_controller(&[0x9F, 0x49, 0x7F]);
        let r = rb(&out);
        assert!(r.contains(&vec![0x90, ddj::VINYL_STATE, 0]));
        assert!(r.contains(&vec![0x90, ddj::JOG_TOUCH, 0]), "held touch is released");
        assert!(ctl(&out).contains(&vec![0x9F, 0x49, LED_DIM]));
        assert!(!e.snapshot().vinyl);
    }

    #[test]
    fn tempo_is_14_bit_and_inverted() {
        let mut e = engine();
        let out = rb(&e.on_controller(&[0xB0, 0x09, 0x7F]));
        let out2 = rb(&e.on_controller(&[0xB0, 0x29, 0x7F]));
        assert_eq!(out.len(), 2);
        assert_eq!(out2, vec![vec![0xB0, 0x00, 0], vec![0xB0, 0x20, 0]]);
        let mut e = Engine::new(Config { invert_tempo: false, ..Config::default() });
        e.on_controller(&[0xB1, 0x09, 0x40]);
        assert_eq!(rb(&e.on_controller(&[0xB1, 0x29, 0x00])), vec![vec![0xB1, 0x00, 0x40], vec![0xB1, 0x20, 0]]);
    }

    #[test]
    fn knobs_send_hires() {
        let mut e = engine();
        assert_eq!(rb(&e.on_controller(&[0xB1, 0x16, 127])), vec![vec![0xB1, 0x13, 127], vec![0xB1, 0x33, 127]]);
        assert_eq!(rb(&e.on_controller(&[0xBF, 0x08, 64])), vec![vec![0xB6, 0x1F, 64], vec![0xB6, 0x3F, 0]]);
        // filter knob defaults to Color FX, switches to EQ low
        assert_eq!(rb(&e.on_controller(&[0xB0, 0x1A, 0]))[0], vec![0xB6, 0x17, 0]);
        e.on_controller(&[0x9F, 0x48, 0x7F]);
        assert_eq!(rb(&e.on_controller(&[0xB1, 0x1A, 0]))[0], vec![0xB1, 0x0F, 0]);
    }

    #[test]
    fn pads_follow_mode() {
        let mut e = engine();
        assert_eq!(rb(&e.on_controller(&[0x94, 0x14, 0x7F])), vec![vec![0x97, 0x00, 0x7F]]);
        assert_eq!(rb(&e.on_controller(&[0x84, 0x14, 0x00])), vec![vec![0x97, 0x00, 0x00]]);
        assert_eq!(rb(&e.on_controller(&[0x95, 0x1F, 0x7F])), vec![vec![0x98, 0x0B, 0x7F]]);
        // hot cue LED from rekordbox
        assert_eq!(ctl(&e.on_rekordbox(&[0x97, 0x01, 0x7F])), vec![vec![0x94, 0x15, 0x7F]]);
        // MODE -> Beat Jump
        let out = e.on_controller(&[0x94, 0x00, 0x7F]);
        assert!(rb(&out).contains(&vec![0x90, 0x69, 0x7F]));
        assert!(ctl(&out).contains(&vec![0x94, 0x15, LED_OFF]), "hot cue LED cleared");
        assert!(ctl(&out).contains(&vec![0x94, 0x02, LED_BRIGHT]));
        assert_eq!(e.snapshot().decks[0].pad_mode, "Beat Jump");
        assert_eq!(rb(&e.on_controller(&[0x94, 0x14, 0x7F])), vec![vec![0x97, 0x40, 0x7F]]);
    }

    #[test]
    fn pad_release_matches_press_after_mode_change() {
        let mut e = engine();
        e.on_controller(&[0x94, 0x14, 0x7F]);
        let out = rb(&e.on_controller(&[0x94, 0x00, 0x7F]));
        assert_eq!(out[0], vec![0x97, 0x00, 0x00]);
        assert!(rb(&e.on_controller(&[0x84, 0x14, 0x00])).is_empty());
    }

    #[test]
    fn remap_binding() {
        let mut cfg = Config::default();
        cfg.bindings.insert("right.acapel".into(), "keylock".into());
        cfg.bindings.insert("browse_press".into(), "play".into()); // per-deck on global: ignored
        let mut e = Engine::new(cfg);
        assert_eq!(rb(&e.on_controller(&[0x91, 0x46, 0x7F])), vec![vec![0x91, 0x1A, 0x7F]]);
        assert_eq!(rb(&e.on_controller(&[0x9F, 0x07, 0x7F])), vec![vec![0x96, 0x41, 0x7F]]);
        assert_eq!(e.binding("browse_press").as_deref(), Some("browse_forward"));
    }

    #[test]
    fn fade_fx_sweeps_filters() {
        let mut e = engine();
        e.on_controller(&[0x9F, 0x46, 0x7F]);
        let out = rb(&e.on_controller(&[0xBF, 0x08, 127]));
        // crossfader + both CFX
        assert_eq!(out.len(), 6);
        assert!(out[2][0] == 0xB6 && out[2][1] == 0x17 && out[2][2] > 64);
        assert_eq!(out[4], vec![0xB6, 0x18, 64]);
        let out = rb(&e.on_controller(&[0x9F, 0x46, 0x7F]));
        assert!(out.contains(&vec![0xB6, 0x17, 64]), "restored when switched off");
    }

    #[test]
    fn browse_and_load() {
        let mut e = engine();
        assert_eq!(rb(&e.on_controller(&[0xBF, 0x00, 0x7F])), vec![vec![0xB6, 0x40, 0x7F]]);
        assert_eq!(rb(&e.on_controller(&[0x9F, 0x03, 0x7F])), vec![vec![0x96, 0x47, 0x7F]]);
        assert_eq!(ctl(&e.on_rekordbox(&[0x9B, 0x01, 0x7F])), vec![vec![0x9F, 0x03, 0x7F]]);
        let mut e = Engine::new(Config { invert_browse: true, ..Config::default() });
        assert_eq!(rb(&e.on_controller(&[0xBF, 0x00, 0x7F])), vec![vec![0xB6, 0x40, 0x01]]);
    }

    #[test]
    fn momentary_led_for_buttons_without_feedback() {
        let mut e = engine();
        let out = e.on_controller(&[0x9F, 0x47, 0x7F]);
        assert_eq!(rb(&out), vec![vec![0x96, 0x65, 0x7F]]);
        assert_eq!(ctl(&out), vec![vec![0x9F, 0x47, LED_BRIGHT]]);
    }

    #[test]
    fn resync_and_disconnect() {
        let mut e = engine();
        e.on_controller(&[0xB0, 0x16, 100]);
        e.on_controller(&[0x90, 0x50, 0x7F]);
        let r = rb(&e.rekordbox_resync());
        assert!(r.contains(&vec![0xB0, 0x13, ddj::split_hires(ddj::to_hires(100)).0]));
        assert!(r.contains(&vec![0x90, ddj::VINYL_STATE, 0x7F]));
        assert_eq!(rb(&e.controller_disconnected()), vec![vec![0x90, ddj::JOG_TOUCH, 0]]);
    }

    #[test]
    fn ignores_garbage() {
        let mut e = engine();
        assert!(e.on_controller(&[]).is_empty());
        assert!(e.on_controller(&[0xF0, 0x01, 0xF7]).is_empty());
        assert!(e.on_controller(&[0x9A, 0x10, 0x7F]).is_empty());
        assert!(e.on_rekordbox(&[0xB0, 0x02, 0x40]).is_empty());
    }

    #[test]
    fn every_default_binding_resolves() {
        for c in mt::controls() {
            let t = ddj::resolve(c.default_action, c.side);
            assert!(t.is_some(), "{} -> {}", c.id, c.default_action);
            assert!(ddj::action_info(c.default_action, c.kind).is_some(), "{} kind", c.id);
        }
        for a in ddj::actions() {
            let side = if a.per_deck { Some(Side::Left) } else { None };
            assert!(ddj::resolve(a.id, side).is_some(), "{}", a.id);
        }
        let json = serde_json::to_string(&crate::catalog()).unwrap();
        assert!(json.contains("left.jog"));
    }

    #[test]
    fn no_input_collisions() {
        let mut seen = std::collections::HashSet::new();
        for c in mt::controls() {
            assert!(seen.insert((c.status, c.data1)), "duplicate {}", c.id);
        }
    }
}
