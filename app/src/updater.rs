//! Keeps RekordJog up to date from its GitHub releases.
//!
//! The newest release's installer is downloaded in the background and its
//! SHA-256 checked against the digest GitHub publishes for it. It is only
//! installed while rekordbox is closed and the controller has been idle,
//! so an update can never interrupt a set. Installing restarts RekordJog.

use crate::runtime::{lock, Runtime};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::AppHandle;

pub const REPO: &str = "palermostest25/RekordJog";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(15);
/// Controller must be untouched this long before an automatic install.
const IDLE_BEFORE_INSTALL_SECS: u64 = 120;

#[derive(Clone, Debug, Serialize, Default, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Downloading { progress: f32 },
    Ready,
    Installing,
    Unsupported,
    Error { message: String },
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct UpdateInfo {
    #[serde(flatten)]
    pub state: UpdateState,
    pub current: String,
    pub latest: Option<String>,
    pub release_url: Option<String>,
    /// Why a ready update hasn't been installed yet.
    pub waiting_for: Option<String>,
    pub checked_mins_ago: Option<u64>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize, Clone)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

pub struct Updater {
    info: Mutex<UpdateInfo>,
    file: Mutex<Option<PathBuf>>,
    last_check: Mutex<Option<Instant>>,
    check_now: AtomicBool,
    install_now: AtomicBool,
}

pub fn version_key(v: &str) -> Vec<u64> {
    v.trim_start_matches(['v', 'V']).split(['.', '-', '+']).map_while(|p| p.parse().ok()).collect()
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    version_key(latest) > version_key(current)
}

/// The installer asset for this platform.
fn pick_asset(assets: &[Asset]) -> Option<&Asset> {
    let want: &dyn Fn(&str) -> bool = if cfg!(target_os = "macos") {
        &|n: &str| n.ends_with(".dmg")
    } else if cfg!(windows) {
        &|n: &str| n.ends_with("-setup.exe")
    } else {
        return None;
    };
    assets.iter().find(|a| want(&a.name.to_ascii_lowercase()))
}

fn agent() -> ureq::Agent {
    let mut b = ureq::AgentBuilder::new();
    if let Ok(tls) = native_tls::TlsConnector::new() {
        b = b.tls_connector(Arc::new(tls));
    }
    b
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(120))
        .user_agent(concat!("RekordJog/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn updates_dir() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("RekordJog").join("updates"))
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

fn digest_ok(path: &Path, asset: &Asset) -> bool {
    let size_ok = std::fs::metadata(path).map(|m| m.len() == asset.size).unwrap_or(false);
    match asset.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) {
        Some(want) => size_ok && sha256_file(path).map(|got| got.eq_ignore_ascii_case(want)).unwrap_or(false),
        None => size_ok,
    }
}

impl Updater {
    pub fn new() -> Arc<Updater> {
        Arc::new(Updater {
            info: Mutex::new(UpdateInfo {
                current: env!("CARGO_PKG_VERSION").to_string(),
                state: if cfg!(any(windows, target_os = "macos")) { UpdateState::Idle } else { UpdateState::Unsupported },
                ..Default::default()
            }),
            file: Mutex::new(None),
            last_check: Mutex::new(None),
            check_now: AtomicBool::new(false),
            install_now: AtomicBool::new(false),
        })
    }

    pub fn info(&self) -> UpdateInfo {
        let mut i = lock(&self.info).clone();
        i.checked_mins_ago = lock(&self.last_check).map(|t| t.elapsed().as_secs() / 60);
        i
    }

    pub fn request_check(&self) {
        self.check_now.store(true, Ordering::Relaxed);
    }

    pub fn request_install(&self) {
        self.install_now.store(true, Ordering::Relaxed);
    }

    fn set(&self, state: UpdateState) {
        lock(&self.info).state = state;
    }

    pub fn start(self: &Arc<Self>, rt: Arc<Runtime>, app: AppHandle) {
        if !cfg!(any(windows, target_os = "macos")) {
            return;
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("rekordjog-updater".into())
            .spawn(move || {
                let started = Instant::now();
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let auto = lock(&rt.settings).auto_update;
                    let manual = me.check_now.swap(false, Ordering::Relaxed);
                    let due = match *lock(&me.last_check) {
                        None => started.elapsed() >= FIRST_CHECK_AFTER,
                        Some(t) => t.elapsed() >= CHECK_EVERY,
                    };
                    let busy = matches!(me.info().state, UpdateState::Downloading { .. } | UpdateState::Installing);
                    if !busy && (manual || (auto && due)) {
                        me.check_and_download();
                    }
                    if lock(&me.info).state == UpdateState::Ready {
                        let status = rt.status();
                        let idle = status.seconds_since_controller_msg.map_or(true, |s| s >= IDLE_BEFORE_INSTALL_SECS);
                        let waiting = if status.rekordbox.running {
                            Some("rekordbox is open — it installs once you close rekordbox".to_string())
                        } else if !idle {
                            Some("waiting until the controller has been idle for 2 minutes".to_string())
                        } else {
                            None
                        };
                        lock(&me.info).waiting_for = waiting.clone();
                        let now = me.install_now.swap(false, Ordering::Relaxed);
                        if now || (auto && waiting.is_none()) {
                            me.install(&app);
                        }
                    } else {
                        me.install_now.store(false, Ordering::Relaxed);
                    }
                }
            })
            .ok();
    }

    fn check_and_download(&self) {
        *lock(&self.last_check) = Some(Instant::now());
        self.set(UpdateState::Checking);
        match self.try_check_and_download() {
            Ok(state) => self.set(state),
            Err(message) => self.set(UpdateState::Error { message }),
        }
    }

    fn try_check_and_download(&self) -> Result<UpdateState, String> {
        let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
        let rel: Release = agent()
            .get(&url)
            .set("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| format!("couldn't reach GitHub ({e})"))?
            .into_json()
            .map_err(|e| format!("unexpected answer from GitHub ({e})"))?;
        let latest = rel.tag_name.trim_start_matches('v').to_string();
        {
            let mut i = lock(&self.info);
            i.latest = Some(latest.clone());
            i.release_url = Some(rel.html_url.clone());
        }
        if rel.draft || rel.prerelease || !is_newer(&latest, env!("CARGO_PKG_VERSION")) {
            return Ok(UpdateState::UpToDate);
        }
        let asset = pick_asset(&rel.assets).ok_or("the new release has no installer for this computer yet")?.clone();
        let dir = updates_dir().ok_or("no cache folder")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // keep only the file we're about to use
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy() != asset.name {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let path = dir.join(&asset.name);
        if !(path.exists() && digest_ok(&path, &asset)) {
            self.download(&asset, &path)?;
            if !digest_ok(&path, &asset) {
                let _ = std::fs::remove_file(&path);
                return Err("the download was corrupted (checksum mismatch); will retry later".into());
            }
        }
        *lock(&self.file) = Some(path);
        Ok(UpdateState::Ready)
    }

    fn download(&self, asset: &Asset, path: &Path) -> Result<(), String> {
        self.set(UpdateState::Downloading { progress: 0.0 });
        let resp = agent().get(&asset.browser_download_url).call().map_err(|e| format!("download failed ({e})"))?;
        let mut reader = resp.into_reader();
        let tmp = path.with_extension("part");
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 1 << 16];
        let mut done = 0u64;
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("download failed ({e})"))?;
            if n == 0 {
                break;
            }
            f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            self.set(UpdateState::Downloading { progress: (done as f32 / asset.size.max(1) as f32).min(1.0) });
        }
        drop(f);
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    fn install(&self, app: &AppHandle) {
        let Some(file) = lock(&self.file).clone() else { return };
        self.set(UpdateState::Installing);
        match install_file(&file) {
            Ok(()) => app.exit(0),
            Err(message) => self.set(UpdateState::Error { message }),
        }
    }
}

/// Starts the platform installer. On success the caller must quit so the
/// new version can replace this one; it relaunches itself afterwards.
#[cfg(windows)]
fn install_file(setup: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // wait for us to exit, install silently, start the new version
    let script = format!(
        "ping -n 3 127.0.0.1 >NUL & \"{}\" /S & start \"\" \"{}\"",
        setup.display(),
        exe.display()
    );
    std::process::Command::new("cmd")
        .raw_arg(format!("/S /C \"{script}\""))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("couldn't start the installer ({e})"))
}

#[cfg(target_os = "macos")]
fn install_file(dmg: &Path) -> Result<(), String> {
    use std::process::Command;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let app = exe
        .ancestors()
        .find(|p| p.extension().map_or(false, |e| e == "app"))
        .ok_or("RekordJog isn't running from an app bundle")?
        .to_path_buf();
    let parent = app.parent().ok_or("bad app location")?.to_path_buf();
    let mnt = std::env::temp_dir().join(format!("rekordjog-update-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&mnt);
    let run = |cmd: &mut Command| -> Result<(), String> {
        let out = cmd.output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    };
    run(Command::new("hdiutil").args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint"]).arg(&mnt).arg(dmg))
        .map_err(|e| format!("couldn't open the update ({e})"))?;
    let staged = parent.join(".RekordJog-update.app");
    let result = (|| {
        let src = std::fs::read_dir(&mnt)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().map_or(false, |e| e == "app"))
            .ok_or("the update has no app inside")?;
        let _ = std::fs::remove_dir_all(&staged);
        run(Command::new("ditto").arg(&src).arg(&staged))
            .map_err(|e| format!("couldn't copy the update next to RekordJog ({e}). Move RekordJog into your Applications folder and try again."))
    })();
    let _ = run(Command::new("hdiutil").args(["detach", "-force"]).arg(&mnt));
    let _ = std::fs::remove_dir(&mnt);
    result?;
    let _ = Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(&staged).output();
    let old = parent.join(".RekordJog-old.app");
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(&app, &old).map_err(|e| format!("couldn't replace RekordJog ({e})"))?;
    if let Err(e) = std::fs::rename(&staged, &app) {
        let _ = std::fs::rename(&old, &app);
        return Err(format!("couldn't replace RekordJog ({e})"));
    }
    let _ = std::fs::remove_dir_all(&old);
    Command::new("sh")
        .arg("-c")
        .arg(format!("sleep 2; open \"{}\"", app.display()))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn install_file(_: &Path) -> Result<(), String> {
    Err("automatic updates aren't available on this system".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("v2.0.10", "2.0.9"));
        assert!(is_newer("2.1.0", "2.0.99"));
        assert!(!is_newer("v2.0.1", "2.0.1"));
        assert!(!is_newer("2.0.0", "2.0.1"));
    }

    #[test]
    fn checksum() {
        let p = std::env::temp_dir().join(format!("rj-sum-{}", std::process::id()));
        std::fs::write(&p, b"hello").unwrap();
        let asset = |digest: &str| Asset {
            name: "x".into(),
            browser_download_url: String::new(),
            size: 5,
            digest: Some(digest.into()),
        };
        assert!(digest_ok(&p, &asset("sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")));
        assert!(!digest_ok(&p, &asset("sha256:00")));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn release_json_parses() {
        let json = r#"{"tag_name":"v2.0.1","html_url":"u","assets":[
            {"name":"RekordJog_2.0.1_universal.dmg","browser_download_url":"a","size":1,"digest":"sha256:ab"},
            {"name":"RekordJog_2.0.1_x64-setup.exe","browser_download_url":"b","size":2}]}"#;
        let r: Release = serde_json::from_str(json).unwrap();
        assert_eq!(r.assets.len(), 2);
        assert!(!r.draft);
    }
}
