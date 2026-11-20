//! Turns the runtime status into a plain-English checklist with fixes.

use crate::rekordbox::MappingState;
use crate::runtime::{Status, VportStatus};
use crate::vport::VportError;
use serde::Serialize;

pub const LOOPMIDI_URL: &str = "https://www.tobias-erichsen.de/software/loopmidi.html";
pub const REKORDBOX_URL: &str = "https://rekordbox.com/en/download/";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Ok,
    Pending,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Fix {
    OpenUrl { label: String, url: String },
    LaunchRekordbox { label: String },
    Rescan { label: String },
}

#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub state: State,
    pub title: String,
    pub detail: String,
    pub fix: Option<Fix>,
}

fn check(id: &'static str, state: State, title: impl Into<String>, detail: impl Into<String>, fix: Option<Fix>) -> Check {
    Check { id, state, title: title.into(), detail: detail.into(), fix }
}

pub fn checks(s: &Status) -> Vec<Check> {
    let mut v = Vec::new();

    // 1. Controller
    v.push(match (&s.controller, &s.controller_error) {
        (Some(name), _) if s.controller_has_output => {
            check("controller", State::Ok, "Mixtrack Go connected", format!("Using MIDI port “{name}”."), None)
        }
        (Some(name), _) => check(
            "controller",
            State::Warn,
            "Mixtrack Go connected (no LEDs)",
            format!("Receiving from “{name}”, but its LED output could not be opened. Controls still work."),
            None,
        ),
        (None, Some(err)) => check(
            "controller",
            State::Error,
            "Mixtrack Go found but can't be opened",
            err.clone(),
            Some(Fix::Rescan { label: "Try again".into() }),
        ),
        (None, None) => {
            let others: Vec<&String> = s.midi_inputs.iter().take(6).collect();
            let mut detail = "Plug it into this computer with a USB-C data cable (Bluetooth is only for phones/tablets). \
                              RekordJog connects automatically."
                .to_string();
            if !others.is_empty() {
                detail.push_str(&format!(
                    " Other MIDI devices seen: {}. If one of these is your controller, choose it under Settings → Controller port.",
                    others.iter().map(|s| format!("“{s}”")).collect::<Vec<_>>().join(", ")
                ));
            }
            check("controller", State::Pending, "Waiting for your Mixtrack Go", detail, None)
        }
    });

    // 2. Virtual DDJ-SX
    v.push(match &s.vport {
        VportStatus::Starting => check("vport", State::Pending, "Creating virtual DDJ-SX…", "", None),
        VportStatus::Ready { kind, feedback: true } => check(
            "vport",
            State::Ok,
            "Virtual DDJ-SX ready",
            format!("rekordbox will see a “PIONEER DDJ-SX” ({kind})."),
            None,
        ),
        VportStatus::Ready { kind, feedback: false } => check(
            "vport",
            State::Warn,
            "Virtual DDJ-SX in basic mode",
            format!(
                "Using an {kind}. Controls work but LEDs can't. For full support, open loopMIDI, delete the \
                 “PIONEER DDJ-SX” port and quit loopMIDI — RekordJog creates its own port automatically."
            ),
            Some(Fix::Rescan { label: "Check again".into() }),
        ),
        VportStatus::Error { error: VportError::DriverMissing } => check(
            "vport",
            State::Error,
            "One-time install: virtual MIDI driver",
            "Windows can't create virtual MIDI devices on its own. Install the free loopMIDI (it contains the \
             driver RekordJog uses). You don't need to open loopMIDI or create any ports — RekordJog does that \
             itself and will pick the driver up automatically once it's installed.",
            Some(Fix::OpenUrl { label: "Download loopMIDI".into(), url: LOOPMIDI_URL.into() }),
        ),
        VportStatus::Error { error: VportError::NameInUse } => check(
            "vport",
            State::Error,
            "“PIONEER DDJ-SX” name already taken",
            "Another program (usually loopMIDI) already has a port with this name. Open loopMIDI, remove that \
             port, then press Check again.",
            Some(Fix::Rescan { label: "Check again".into() }),
        ),
        VportStatus::Error { error: VportError::Other(e) } => check(
            "vport",
            State::Error,
            "Couldn't create the virtual DDJ-SX",
            e.clone(),
            Some(Fix::Rescan { label: "Try again".into() }),
        ),
    });

    // 3. rekordbox installed
    let rb = &s.rekordbox;
    if rb.installed {
        v.push(check(
            "rekordbox",
            State::Ok,
            format!("rekordbox {} installed", rb.version.clone().unwrap_or_default()).replace("  ", " "),
            rb.app_path.clone().unwrap_or_default(),
            None,
        ));
    } else if cfg!(any(windows, target_os = "macos")) {
        v.push(check(
            "rekordbox",
            State::Error,
            "rekordbox not found",
            "Install rekordbox 6 or 7. RekordJog looks in the standard install folder.",
            Some(Fix::OpenUrl { label: "Get rekordbox".into(), url: REKORDBOX_URL.into() }),
        ));
    }

    // 4. DDJ-SX mapping
    if rb.installed {
        let path = rb.mapping_path.clone().unwrap_or_default();
        match rb.mapping {
            MappingState::Original => v.push(check(
                "mapping",
                State::Ok,
                "rekordbox's DDJ-SX mapping is intact",
                "Nothing to import — RekordJog uses rekordbox's built-in DDJ-SX support.",
                None,
            )),
            MappingState::OldRekordJog => v.push(check(
                "mapping",
                State::Error,
                "rekordbox's DDJ-SX file was replaced by the old RekordJog",
                format!(
                    "The old Python version asked you to overwrite {path}. The new RekordJog needs the original \
                     file. Reinstall rekordbox (or restore your backup of that file) to fix it."
                ),
                Some(Fix::OpenUrl { label: "Download rekordbox".into(), url: REKORDBOX_URL.into() }),
            )),
            MappingState::Modified => v.push(check(
                "mapping",
                State::Warn,
                "rekordbox's DDJ-SX mapping has been edited",
                format!("{path} isn't the stock file. If some buttons do the wrong thing, reinstall rekordbox."),
                None,
            )),
            MappingState::NotFound => {}
        }
        if !rb.custom_mappings.is_empty() {
            v.push(check(
                "custom_mapping",
                State::Info,
                "Saved custom DDJ-SX mapping found",
                format!(
                    "rekordbox has a MIDI Learn file for the DDJ-SX ({}). If controls behave oddly, open rekordbox's \
                     MIDI window, select PIONEER DDJ-SX and click “Default”.",
                    rb.custom_mappings[0]
                ),
                None,
            ));
        }
    }

    // 5. rekordbox talking to us
    let feedback = matches!(s.vport, VportStatus::Ready { feedback: true, .. });
    if rb.running {
        let secs = s.rekordbox_running_secs.unwrap_or(0);
        if s.rekordbox_linked {
            v.push(check("link", State::Ok, "rekordbox is connected", "rekordbox is talking to the virtual DDJ-SX.", None));
        } else if !feedback {
            v.push(check(
                "link",
                State::Info,
                "rekordbox is running",
                "In basic mode RekordJog can't confirm the connection. Check that rekordbox shows a DDJ-SX.",
                None,
            ));
        } else if secs < 20 {
            v.push(check("link", State::Pending, "Waiting for rekordbox to connect…", "This takes a few seconds after rekordbox opens.", None));
        } else if s.rekordbox_open_before_port {
            v.push(check(
                "link",
                State::Warn,
                "Restart rekordbox",
                "rekordbox was already open before the virtual DDJ-SX existed, so it hasn't noticed it. Quit rekordbox \
                 and open it again (RekordJog can stay open).",
                None,
            ));
        } else {
            v.push(check(
                "link",
                State::Warn,
                "rekordbox hasn't connected yet",
                "Make sure rekordbox is in PERFORMANCE mode (top-left menu). If it still doesn't connect, quit and \
                 reopen rekordbox. Note: rekordbox needs a plan that allows DJ controllers (Core or higher) for a DDJ-SX.",
                None,
            ));
        }
    } else if rb.installed {
        v.push(check(
            "link",
            State::Pending,
            "Open rekordbox",
            "Start rekordbox once your controller shows as connected. It will find the “DDJ-SX” automatically.",
            Some(Fix::LaunchRekordbox { label: "Open rekordbox".into() }),
        ));
    }
    v
}

pub fn overall(checks: &[Check]) -> State {
    checks.iter().map(|c| c.state).max_by_key(|s| match s {
        State::Ok | State::Info => 0,
        State::Pending => 1,
        State::Warn => 2,
        State::Error => 3,
    }).unwrap_or(State::Ok)
}

/// Text report users can paste into a bug report.
pub fn report(s: &Status, checks: &[Check]) -> String {
    let mut r = String::new();
    r.push_str(&format!("RekordJog {} on {} ({})\n", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH));
    for c in checks {
        r.push_str(&format!("[{:?}] {} — {}\n", c.state, c.title, c.detail));
    }
    r.push_str(&format!("MIDI inputs: {:?}\n", s.midi_inputs));
    r.push_str(&format!("Virtual port: {:?}\n", s.vport));
    r.push_str(&format!(
        "rekordbox: installed={} version={:?} running={} linked={} mapping={:?}\n",
        s.rekordbox.installed, s.rekordbox.version, s.rekordbox.running, s.rekordbox_linked, s.rekordbox.mapping
    ));
    r.push_str(&format!(
        "Last controller msg: {:?}s ago, last rekordbox msg: {:?}s ago\n",
        s.seconds_since_controller_msg, s.seconds_since_rekordbox_msg
    ));
    r
}
