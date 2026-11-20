//! Finds rekordbox, checks its DDJ-SX mapping and whether it is running.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MappingState {
    /// Stock DDJ-SX mapping: everything will work.
    Original,
    /// Replaced by the old Python RekordJog's stripped-down file.
    OldRekordJog,
    /// Some other edit.
    Modified,
    /// Could not find the file (newer rekordbox may store it elsewhere).
    #[default]
    NotFound,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct RekordboxInfo {
    pub installed: bool,
    pub app_path: Option<String>,
    pub version: Option<String>,
    pub mapping: MappingState,
    pub mapping_path: Option<String>,
    pub running: bool,
    /// Saved MIDI Learn files for the DDJ-SX that may override the default.
    pub custom_mappings: Vec<String>,
}

#[allow(dead_code)]
pub const MAPPING_FILE: &str = "PIONEER DDJ-SX.midi.csv";

fn version_key(v: &str) -> Vec<u32> {
    v.split(|c: char| !c.is_ascii_digit()).filter_map(|p| p.parse().ok()).collect()
}

fn check_mapping(path: &Path) -> MappingState {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            if text.contains("JogScratch,JogScratch,JogRotate,B00A") {
                MappingState::OldRekordJog
            } else if text.contains("JogScratch,JogScratch,JogRotate,B022") && text.contains("PlayPause") {
                MappingState::Original
            } else {
                MappingState::Modified
            }
        }
        Err(_) => MappingState::NotFound,
    }
}

fn find_custom_mappings(root: &Path, depth: usize, out: &mut Vec<String>) {
    if depth == 0 || out.len() > 10 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            find_custom_mappings(&p, depth - 1, out);
        } else if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            let lower = name.to_ascii_lowercase();
            if lower.contains("ddj-sx") && lower.ends_with(".csv") {
                out.push(p.display().to_string());
            }
        }
    }
}

struct Install {
    app: PathBuf,
    version: Option<String>,
    mapping: PathBuf,
}

#[cfg(target_os = "macos")]
fn installs() -> Vec<Install> {
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(h) = dirs::home_dir() {
        roots.push(h.join("Applications"));
    }
    let mut apps = Vec::new();
    for root in roots {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            if !name.starts_with("rekordbox") {
                continue;
            }
            let p = e.path();
            if name.ends_with(".app") {
                apps.push(p);
            } else if p.join("rekordbox.app").is_dir() {
                apps.push(p.join("rekordbox.app"));
            }
        }
    }
    apps.into_iter()
        .map(|app| {
            let version = plist::Value::from_file(app.join("Contents/Info.plist"))
                .ok()
                .and_then(|v| {
                    v.as_dictionary()?.get("CFBundleShortVersionString")?.as_string().map(str::to_string)
                });
            let mapping = app.join("Contents/Resources/MidiMappings").join(MAPPING_FILE);
            Install { app, version, mapping }
        })
        .collect()
}

#[cfg(windows)]
fn installs() -> Vec<Install> {
    let mut roots = Vec::new();
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Ok(p) = std::env::var(var) {
            roots.push(PathBuf::from(&p).join("Pioneer"));
            roots.push(PathBuf::from(&p).join("AlphaTheta"));
            roots.push(PathBuf::from(p));
        }
    }
    roots.dedup();
    let mut out = Vec::new();
    for root in roots {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.to_ascii_lowercase().starts_with("rekordbox") {
                continue;
            }
            let dir = e.path();
            let exe = dir.join("rekordbox.exe");
            if exe.is_file() {
                let v = name.trim_start_matches(|c: char| !c.is_ascii_digit()).trim().to_string();
                out.push(Install {
                    app: exe,
                    version: if v.is_empty() { None } else { Some(v) },
                    mapping: dir.join("MidiMappings").join(MAPPING_FILE),
                });
            }
        }
    }
    out
}

#[cfg(not(any(windows, target_os = "macos")))]
fn installs() -> Vec<Install> {
    Vec::new()
}

fn user_data_dirs() -> Vec<PathBuf> {
    #[allow(unused_mut)]
    let mut v = Vec::new();
    #[cfg(target_os = "macos")]
    if let Some(h) = dirs::home_dir() {
        v.push(h.join("Library/Pioneer"));
    }
    #[cfg(windows)]
    if let Some(d) = dirs::data_dir() {
        v.push(d.join("Pioneer"));
    }
    v
}

/// Look for rekordbox on disk (cheap; no process scan).
pub fn detect_install() -> RekordboxInfo {
    let mut list = installs();
    list.sort_by(|a, b| {
        version_key(a.version.as_deref().unwrap_or("")).cmp(&version_key(b.version.as_deref().unwrap_or("")))
    });
    let mut info = RekordboxInfo::default();
    if let Some(best) = list.pop() {
        info.installed = true;
        info.app_path = Some(best.app.display().to_string());
        info.version = best.version;
        info.mapping = check_mapping(&best.mapping);
        info.mapping_path = Some(best.mapping.display().to_string());
    }
    for d in user_data_dirs() {
        find_custom_mappings(&d, 4, &mut info.custom_mappings);
    }
    info
}

/// Is the rekordbox application (not its background agent) running?
pub fn is_running(sys: &mut sysinfo::System) -> bool {
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    sys.processes().values().any(|p| {
        let n = p.name().to_string_lossy().to_ascii_lowercase();
        n == "rekordbox" || n == "rekordbox.exe"
    })
}

pub fn launch(info: &RekordboxInfo) -> Result<(), String> {
    let Some(path) = &info.app_path else { return Err("rekordbox is not installed".into()) };
    let res = if cfg!(target_os = "macos") {
        Command::new("open").arg("-a").arg(path).spawn()
    } else {
        Command::new(path).spawn()
    };
    res.map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_detection() {
        let dir = std::env::temp_dir().join(format!("rj-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("m.csv");
        let original = include_str!("../../docs/PIONEER DDJ-SX.midi.original.csv");
        std::fs::write(&f, original).unwrap();
        assert_eq!(check_mapping(&f), MappingState::Original);
        std::fs::write(&f, "JogScratch,JogScratch,JogRotate,B00A,1,2").unwrap();
        assert_eq!(check_mapping(&f), MappingState::OldRekordJog);
        std::fs::write(&f, "hello").unwrap();
        assert_eq!(check_mapping(&f), MappingState::Modified);
        assert_eq!(check_mapping(&dir.join("nope.csv")), MappingState::NotFound);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn versions_sort() {
        assert!(version_key("7.1.2") > version_key("6.8.5"));
        assert!(version_key("7.10.0") > version_key("7.9.9"));
    }
}
