//! Owns the engine and every MIDI connection, and keeps them healthy:
//! auto-connects the controller when it is plugged in, recreates the
//! virtual DDJ-SX if needed and watches rekordbox.

use crate::rekordbox::{self, RekordboxInfo};
use crate::settings::Settings;
use crate::vport::{self, VirtualPort, VportError};
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use rekordjog_core::{Dest, Engine, Out, Snapshot, UiEvent};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

const LOG_CAP: usize = 400;
const EVENT_CAP: usize = 200;

/// Lock that survives a panic in another thread instead of poisoning the app.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Debug, Serialize, Default)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VportStatus {
    #[default]
    Starting,
    Ready { kind: String, feedback: bool },
    Error { error: VportError },
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct Status {
    pub controller: Option<String>,
    pub controller_has_output: bool,
    /// MIDI outputs RekordJog sends the controller's LED messages to.
    pub controller_outputs: Vec<String>,
    /// Every MIDI output the OS reports.
    pub midi_outputs: Vec<String>,
    pub controller_messages_sent: u64,
    pub controller_send_errors: u64,
    pub last_send_error: Option<String>,
    pub controller_error: Option<String>,
    /// Every MIDI input the OS reports (for manual selection).
    pub midi_inputs: Vec<String>,
    pub vport: VportStatus,
    pub rekordbox: RekordboxInfo,
    /// rekordbox was already open when the virtual DDJ-SX appeared.
    pub rekordbox_open_before_port: bool,
    /// rekordbox has sent at least one message to the virtual DDJ-SX.
    pub rekordbox_linked: bool,
    /// How long rekordbox has been running (seconds), if it is.
    pub rekordbox_running_secs: Option<u64>,
    pub seconds_since_controller_msg: Option<u64>,
    pub seconds_since_rekordbox_msg: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LogLine {
    pub seq: u64,
    pub t: u64,
    /// "ctl>" from controller, ">rb" to rekordbox, "rb>" from rekordbox, ">ctl" to controller
    pub dir: &'static str,
    pub hex: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SeqEvent {
    pub seq: u64,
    #[serde(flatten)]
    pub event: UiEvent,
}

pub struct Core {
    pub engine: Engine,
    ctl_out: Vec<(String, MidiOutputConnection)>,
    vport: Option<Box<dyn VirtualPort>>,
}

pub struct Runtime {
    pub core: Mutex<Core>,
    pub status: Mutex<Status>,
    pub settings: Mutex<Settings>,
    log: Mutex<VecDeque<LogLine>>,
    events: Mutex<VecDeque<SeqEvent>>,
    seq: AtomicU64,
    pub monitor: AtomicBool,
    start: Instant,
    last_ctl_ms: AtomicU64,
    last_rb_ms: AtomicU64,
    rb_linked: AtomicBool,
    needs_resync: AtomicBool,
    force_rescan: AtomicBool,
    ctl_sent: AtomicU64,
    ctl_errors: AtomicU64,
    last_send_error: Mutex<Option<String>>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

fn normalize(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase()
}

/// Is this MIDI port a Mixtrack Go?
pub fn looks_like_mixtrack_go(name: &str) -> bool {
    !vport::is_own_port(name) && normalize(name).contains("mixtrackgo")
}

impl Runtime {
    pub fn new(settings: Settings) -> Arc<Runtime> {
        Arc::new(Runtime {
            core: Mutex::new(Core { engine: Engine::new(settings.engine.clone()), ctl_out: Vec::new(), vport: None }),
            status: Mutex::new(Status::default()),
            settings: Mutex::new(settings),
            log: Mutex::new(VecDeque::new()),
            events: Mutex::new(VecDeque::new()),
            seq: AtomicU64::new(1),
            monitor: AtomicBool::new(false),
            start: Instant::now(),
            last_ctl_ms: AtomicU64::new(0),
            last_rb_ms: AtomicU64::new(0),
            rb_linked: AtomicBool::new(false),
            needs_resync: AtomicBool::new(false),
            force_rescan: AtomicBool::new(false),
            ctl_sent: AtomicU64::new(0),
            ctl_errors: AtomicU64::new(0),
            last_send_error: Mutex::new(None),
        })
    }

    pub fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64 + 1
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    fn log(&self, dir: &'static str, bytes: &[u8]) {
        if !self.monitor.load(Ordering::Relaxed) {
            return;
        }
        let line = LogLine { seq: self.next_seq(), t: self.now_ms(), dir, hex: hex(bytes) };
        let mut log = lock(&self.log);
        if log.len() >= LOG_CAP {
            log.pop_front();
        }
        log.push_back(line);
    }

    fn dispatch(&self, core: &mut Core, outs: Vec<Out>) {
        for o in outs {
            match o.dest {
                Dest::Rekordbox => {
                    if let Some(p) = core.vport.as_mut() {
                        p.send(&o.bytes);
                        self.log(">rb", &o.bytes);
                    }
                }
                Dest::Controller => {
                    if core.ctl_out.is_empty() {
                        continue;
                    }
                    for (name, c) in core.ctl_out.iter_mut() {
                        match c.send(&o.bytes) {
                            Ok(()) => {
                                self.ctl_sent.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(e) => {
                                self.ctl_errors.fetch_add(1, Ordering::Relaxed);
                                *lock(&self.last_send_error) = Some(format!("{name}: {e} (message {})", hex(&o.bytes)));
                            }
                        }
                    }
                    self.log(">ctl", &o.bytes);
                }
            }
        }
    }

    /// Run `f` against the engine and send whatever it produces.
    pub fn with_engine<R>(&self, f: impl FnOnce(&mut Engine) -> (Vec<Out>, R)) -> R {
        let mut core = lock(&self.core);
        let (outs, r) = f(&mut core.engine);
        self.dispatch(&mut core, outs);
        r
    }

    fn on_controller(&self, msg: &[u8]) {
        self.last_ctl_ms.store(self.now_ms(), Ordering::Relaxed);
        self.log("ctl>", msg);
        let mut core = lock(&self.core);
        let outs = core.engine.on_controller(msg);
        self.dispatch(&mut core, outs);
    }

    fn on_rekordbox(&self, msg: &[u8]) {
        self.last_rb_ms.store(self.now_ms(), Ordering::Relaxed);
        if !self.rb_linked.swap(true, Ordering::Relaxed) {
            self.needs_resync.store(true, Ordering::Relaxed);
        }
        self.log("rb>", msg);
        let mut core = lock(&self.core);
        let outs = core.engine.on_rekordbox(msg);
        self.dispatch(&mut core, outs);
    }

    pub fn rescan(&self) {
        self.force_rescan.store(true, Ordering::Relaxed);
    }

    pub fn events_since(&self, since: u64) -> Vec<SeqEvent> {
        lock(&self.events).iter().filter(|e| e.seq > since).cloned().collect()
    }

    pub fn log_since(&self, since: u64) -> Vec<LogLine> {
        lock(&self.log).iter().filter(|l| l.seq > since).cloned().collect()
    }

    pub fn clear_log(&self) {
        lock(&self.log).clear();
    }

    pub fn snapshot(&self) -> Snapshot {
        lock(&self.core).engine.snapshot()
    }

    pub fn status(&self) -> Status {
        let mut s = lock(&self.status).clone();
        let now = self.now_ms();
        let ago = |t: u64| if t == 0 { None } else { Some(now.saturating_sub(t) / 1000) };
        s.seconds_since_controller_msg = ago(self.last_ctl_ms.load(Ordering::Relaxed));
        s.seconds_since_rekordbox_msg = ago(self.last_rb_ms.load(Ordering::Relaxed));
        s.rekordbox_linked = self.rb_linked.load(Ordering::Relaxed);
        s.controller_messages_sent = self.ctl_sent.load(Ordering::Relaxed);
        s.controller_send_errors = self.ctl_errors.load(Ordering::Relaxed);
        s.last_send_error = lock(&self.last_send_error).clone();
        s
    }

    /// Start the background threads.
    pub fn start(self: &Arc<Self>) {
        let rt = self.clone();
        thread::Builder::new().name("rekordjog-ticker".into()).spawn(move || rt.ticker()).ok();
        let rt = self.clone();
        thread::Builder::new().name("rekordjog-supervisor".into()).spawn(move || rt.supervisor()).ok();
    }

    fn ticker(self: Arc<Self>) {
        loop {
            let events = {
                let mut core = lock(&self.core);
                let mut outs = core.engine.tick(self.now_ms());
                if core.vport.is_some() && self.needs_resync.swap(false, Ordering::Relaxed) {
                    outs.extend(core.engine.rekordbox_resync());
                }
                self.dispatch(&mut core, outs);
                core.engine.drain_events()
            };
            if !events.is_empty() {
                let mut q = lock(&self.events);
                for event in events {
                    if q.len() >= EVENT_CAP {
                        q.pop_front();
                    }
                    q.push_back(SeqEvent { seq: self.next_seq(), event });
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn ensure_vport(self: &Arc<Self>) {
        if lock(&self.core).vport.is_some() {
            return;
        }
        let rt = self.clone();
        let res = vport::create(Box::new(move |msg| rt.on_rekordbox(msg)));
        let rb_running = lock(&self.status).rekordbox.running;
        let status = match res {
            Ok(p) => {
                let st = VportStatus::Ready { kind: p.kind().to_string(), feedback: p.has_feedback() };
                self.rb_linked.store(false, Ordering::Relaxed);
                self.needs_resync.store(true, Ordering::Relaxed);
                lock(&self.core).vport = Some(p);
                lock(&self.status).rekordbox_open_before_port = rb_running;
                st
            }
            Err(error) => VportStatus::Error { error },
        };
        lock(&self.status).vport = status;
    }

    fn supervisor(self: Arc<Self>) {
        let mut sys = sysinfo::System::new();
        let mut ctl_in: Option<MidiInputConnection<()>> = None;
        let mut ctl_name: Option<String> = None;
        let mut last_vport_try: Option<Instant> = None;
        let mut last_install_scan: Option<Instant> = None;
        let mut last_proc_scan: Option<Instant> = None;
        let mut rb_running_since: Option<Instant> = None;
        let mut last_connect_error: Option<(String, Instant)> = None;

        loop {
            let forced = self.force_rescan.swap(false, Ordering::Relaxed);
            let due = |t: &Option<Instant>, secs: u64| forced || t.map_or(true, |t| t.elapsed() >= Duration::from_secs(secs));

            // rekordbox
            if due(&last_install_scan, 20) {
                last_install_scan = Some(Instant::now());
                let info = rekordbox::detect_install();
                let mut st = lock(&self.status);
                let running = st.rekordbox.running;
                st.rekordbox = RekordboxInfo { running, ..info };
            }
            if due(&last_proc_scan, 2) {
                last_proc_scan = Some(Instant::now());
                let running = rekordbox::is_running(&mut sys);
                if running && rb_running_since.is_none() {
                    rb_running_since = Some(Instant::now());
                }
                if !running {
                    rb_running_since = None;
                    if self.rb_linked.swap(false, Ordering::Relaxed) {
                        // rekordbox quit; next launch gets a fresh sync
                        self.needs_resync.store(false, Ordering::Relaxed);
                    }
                    lock(&self.status).rekordbox_open_before_port = false;
                }
                let mut st = lock(&self.status);
                st.rekordbox.running = running;
                st.rekordbox_running_secs = rb_running_since.map(|t| t.elapsed().as_secs());
            } else if let Some(t) = rb_running_since {
                lock(&self.status).rekordbox_running_secs = Some(t.elapsed().as_secs());
            }

            // virtual DDJ-SX
            if due(&last_vport_try, 3) {
                last_vport_try = Some(Instant::now());
                self.ensure_vport();
            }

            // controller
            let names: Vec<String> = match MidiInput::new("RekordJog scan") {
                Ok(m) => m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect(),
                Err(_) => Vec::new(),
            };
            let override_name = lock(&self.settings).controller_port.clone().filter(|n| !n.is_empty());
            let desired = match &override_name {
                Some(n) => names.iter().find(|x| *x == n).cloned(),
                None => names
                    .iter()
                    .filter(|n| looks_like_mixtrack_go(n))
                    .min_by_key(|n| n.to_ascii_lowercase().contains("bluetooth"))
                    .cloned(),
            };
            if ctl_name.is_some() && (desired != ctl_name || forced) {
                self.disconnect_controller(&mut ctl_in);
                ctl_name = None;
            }
            if ctl_name.is_none() {
                if let Some(name) = &desired {
                    let retry_ok = match &last_connect_error {
                        Some((n, t)) if n == name => forced || t.elapsed() >= Duration::from_secs(3),
                        _ => true,
                    };
                    if retry_ok {
                        match self.connect_controller(name) {
                            Ok(conn) => {
                                ctl_in = Some(conn);
                                ctl_name = Some(name.clone());
                                last_connect_error = None;
                            }
                            Err(e) => {
                                lock(&self.status).controller_error = Some(e);
                                last_connect_error = Some((name.clone(), Instant::now()));
                            }
                        }
                    }
                } else {
                    lock(&self.status).controller_error = None;
                }
            }
            let out_names: Vec<String> = match MidiOutput::new("RekordJog scan") {
                Ok(m) => m.ports().iter().filter_map(|p| m.port_name(p).ok()).filter(|n| !vport::is_own_port(n)).collect(),
                Err(_) => Vec::new(),
            };
            {
                let mut st = lock(&self.status);
                st.midi_outputs = out_names;
                st.midi_inputs = names.into_iter().filter(|n| !vport::is_own_port(n)).collect();
                st.controller = ctl_name.clone();
            }

            thread::sleep(Duration::from_millis(700));
        }
    }

    fn connect_controller(self: &Arc<Self>, name: &str) -> Result<MidiInputConnection<()>, String> {
        let busy = "Your Mixtrack Go is in use by another app. Close Serato, djay, VirtualDJ or Mixxx \
                    (or rekordbox, then reopen it after RekordJog connects).";
        let mut input = MidiInput::new("RekordJog").map_err(|e| e.to_string())?;
        input.ignore(Ignore::All);
        let port = input
            .ports()
            .into_iter()
            .find(|p| input.port_name(p).ok().as_deref() == Some(name))
            .ok_or("controller disappeared")?;
        let rt = self.clone();
        let conn = input
            .connect(&port, "RekordJog Mixtrack Go", move |_t, msg, _| rt.on_controller(msg), ())
            .map_err(|_| busy.to_string())?;

        // LED outputs: every output that belongs to the controller. Some
        // systems list it as several ports, and the lights may only listen
        // on one of them, so send to all.
        let mut outs: Vec<(String, MidiOutputConnection)> = Vec::new();
        let mut open_errors = Vec::new();
        self.ctl_errors.store(0, Ordering::Relaxed);
        *lock(&self.last_send_error) = None;
        if let Ok(probe) = MidiOutput::new("RekordJog") {
            let wanted: Vec<String> = probe
                .ports()
                .iter()
                .filter_map(|p| probe.port_name(p).ok())
                .filter(|n| !vport::is_own_port(n) && (n == name || normalize(n) == normalize(name) || looks_like_mixtrack_go(n)))
                .collect();
            for (i, out_name) in wanted.iter().enumerate() {
                let Ok(o) = MidiOutput::new("RekordJog") else { continue };
                // pick the i-th port with this name, so duplicates each get opened
                let same: Vec<_> = o.ports().into_iter().filter(|p| o.port_name(p).ok().as_deref() == Some(out_name.as_str())).collect();
                let nth = wanted[..i].iter().filter(|n| *n == out_name).count();
                let Some(port) = same.get(nth) else { continue };
                match o.connect(port, "RekordJog Mixtrack Go LEDs") {
                    Ok(c) => outs.push((out_name.clone(), c)),
                    Err(e) => open_errors.push(format!("{out_name}: {e}")),
                }
            }
        }
        if !open_errors.is_empty() {
            *lock(&self.last_send_error) = Some(format!("couldn't open {}", open_errors.join(", ")));
        }

        let has_out = !outs.is_empty();
        let out_names: Vec<String> = outs.iter().map(|(n, _)| n.clone()).collect();
        {
            let mut core = lock(&self.core);
            core.ctl_out = outs;
            let outs = core.engine.controller_connected(self.now_ms());
            self.dispatch(&mut core, outs);
        }
        let mut st = lock(&self.status);
        st.controller_has_output = has_out;
        st.controller_outputs = out_names;
        st.controller_error = None;
        Ok(conn)
    }

    fn disconnect_controller(&self, ctl_in: &mut Option<MidiInputConnection<()>>) {
        // Drop the input outside the core lock: its callback may be waiting on it.
        if let Some(c) = ctl_in.take() {
            c.close();
        }
        let mut core = lock(&self.core);
        let outs = core.engine.controller_disconnected();
        self.dispatch(&mut core, outs);
        core.ctl_out.clear();
        drop(core);
        let mut st = lock(&self.status);
        st.controller_has_output = false;
        st.controller_outputs.clear();
    }
}
