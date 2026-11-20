use rekordjog_core::Config;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub engine: Config,
    /// The guided setup has been completed once.
    pub setup_done: bool,
    /// Exact MIDI port name to use instead of auto-detecting a Mixtrack Go.
    pub controller_port: Option<String>,
    /// Closing the window keeps RekordJog running in the tray/menu bar.
    pub keep_running_in_tray: bool,
    pub launch_at_login: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            engine: Config::default(),
            setup_done: false,
            controller_port: None,
            keep_running_in_tray: true,
            launch_at_login: false,
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("RekordJog").join("settings.json"))
}

pub fn load() -> Settings {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(s: &Settings) -> Result<(), String> {
    let p = path().ok_or("no config directory")?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = p.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
}
