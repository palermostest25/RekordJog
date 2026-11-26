#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod diag;
mod rekordbox;
mod runtime;
mod settings;
mod updater;
mod vport;

use runtime::{lock, LogLine, Runtime, SeqEvent, Status};
use serde::Serialize;
use settings::Settings;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

struct AppState(Arc<Runtime>);
struct UpdaterState(Arc<updater::Updater>);

#[derive(Serialize)]
struct Poll {
    status: Status,
    snapshot: rekordjog_core::Snapshot,
    checks: Vec<diag::Check>,
    overall: diag::State,
    events: Vec<SeqEvent>,
    log: Vec<LogLine>,
    platform: &'static str,
    version: &'static str,
    update: updater::UpdateInfo,
}

#[tauri::command]
fn poll(state: State<AppState>, upd: State<UpdaterState>, events_since: u64, log_since: u64) -> Poll {
    let rt = &state.0;
    let status = rt.status();
    let checks = diag::checks(&status);
    Poll {
        overall: diag::overall(&checks),
        checks,
        snapshot: rt.snapshot(),
        events: rt.events_since(events_since),
        log: rt.log_since(log_since),
        status,
        platform: std::env::consts::OS,
        version: env!("CARGO_PKG_VERSION"),
        update: upd.0.info(),
    }
}

#[tauri::command]
fn check_updates(upd: State<UpdaterState>) {
    upd.0.request_check();
}

#[tauri::command]
fn install_update(upd: State<UpdaterState>) {
    upd.0.request_install();
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    lock(&state.0.settings).clone()
}

#[tauri::command]
fn save_settings(app: AppHandle, state: State<AppState>, settings: Settings) -> Result<Settings, String> {
    let rt = &state.0;
    let previous = lock(&rt.settings).clone();
    rt.with_engine(|e| (e.set_config(settings.engine.clone()), ()));
    if previous.controller_port != settings.controller_port {
        rt.rescan();
    }
    if previous.launch_at_login != settings.launch_at_login {
        let al = app.autolaunch();
        let r = if settings.launch_at_login { al.enable() } else { al.disable() };
        if let Err(e) = r {
            eprintln!("autostart: {e}");
        }
    }
    // keep the sanitized engine config
    let mut settings = settings;
    settings.engine = lock(&rt.core).engine.config().clone();
    *lock(&rt.settings) = settings.clone();
    settings::save(&settings)?;
    Ok(settings)
}

#[derive(Serialize)]
struct CatalogPayload {
    catalog: rekordjog_core::Catalog,
    bindings: BTreeMap<String, String>,
}

#[tauri::command]
fn get_catalog(state: State<AppState>) -> CatalogPayload {
    let catalog = rekordjog_core::catalog();
    let core = lock(&state.0.core);
    let bindings = catalog
        .controls
        .iter()
        .filter_map(|c| core.engine.binding(&c.id).map(|b| (c.id.clone(), b)))
        .collect();
    CatalogPayload { catalog, bindings }
}

#[tauri::command]
fn set_monitor(state: State<AppState>, on: bool) {
    state.0.monitor.store(on, Ordering::Relaxed);
    if !on {
        state.0.clear_log();
    }
}

#[tauri::command]
fn rescan(state: State<AppState>) {
    state.0.rescan();
}

#[tauri::command]
fn launch_rekordbox(state: State<AppState>) -> Result<(), String> {
    let info = lock(&state.0.status).rekordbox.clone();
    rekordbox::launch(&info)
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("only https links can be opened".into());
    }
    open::that_detached(url).map_err(|e| e.to_string())
}

#[tauri::command]
fn report(state: State<AppState>) -> String {
    let status = state.0.status();
    diag::report(&status, &diag::checks(&status))
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn main() {
    let settings = settings::load();
    let rt = Runtime::new(settings);
    let rt_setup = rt.clone();
    let updater = updater::Updater::new();
    let updater_setup = updater.clone();
    let rt_events = rt.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .manage(AppState(rt))
        .manage(UpdaterState(updater))
        .invoke_handler(tauri::generate_handler![
            poll,
            get_settings,
            save_settings,
            get_catalog,
            set_monitor,
            rescan,
            launch_rekordbox,
            open_url,
            report,
            quit,
            check_updates,
            install_update
        ])
        .setup(move |app| {
            // CoreMIDI only refreshes its device list on the run loop of the
            // thread that created the first client: make that the main thread.
            #[cfg(target_os = "macos")]
            if let Ok(client) = midir::MidiInput::new("RekordJog") {
                std::mem::forget(client);
            }
            rt_setup.start();
            updater_setup.start(rt_setup.clone(), app.handle().clone());

            let show = MenuItem::with_id(app, "show", "Show RekordJog", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit RekordJog", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("RekordJog")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "show" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        show_main(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            if !std::env::args().any(|a| a == "--hidden") {
                show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(move |window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if lock(&rt_events.settings).keep_running_in_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running RekordJog");
}
