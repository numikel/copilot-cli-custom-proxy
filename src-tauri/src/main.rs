// On Windows in release mode hides the console — the app lives in the tray.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod commands;
mod startup_error;
mod tray;

use proxy_core::{AppState, Config, RuntimeConfig};
use startup_error::show_startup_error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{async_runtime::JoinHandle, AppHandle, Manager};

/// Handle to the background proxy task, kept in Tauri-managed state so the
/// listen-address command can abort and respawn it. Lives in the GUI crate to
/// keep `proxy-core` free of Tauri dependencies.
pub struct ProxyTask(pub Mutex<Option<JoinHandle<()>>>);

/// Directories searched for `config.json` / `config.toml` / `ui_state.json`:
/// next to the executable, then the current working directory.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
        }
    }
    dirs.push(PathBuf::from("."));
    dirs
}

/// Resolves the runtime config at startup:
/// 1. an existing `config.json` (the source of truth), else
/// 2. a legacy `config.toml`, migrated and seeded into `config.json`, else
/// 3. built-in defaults.
///
/// Returns the config, the directory its `config.json` lives in, and whether
/// the app needs first-run setup (no usable endpoint yet → show settings).
fn resolve_config() -> (RuntimeConfig, PathBuf, bool) {
    let dirs = candidate_dirs();

    for dir in &dirs {
        let json = dir.join("config.json");
        if json.exists() {
            if let Some(cfg) = RuntimeConfig::load(&json) {
                tracing::info!("loaded config.json from {}", json.display());
                let needs_setup = !cfg.is_configured();
                return (cfg, dir.clone(), needs_setup);
            }
            tracing::warn!("config.json at {} is unreadable — ignoring", json.display());
        }
    }

    for dir in &dirs {
        let toml = dir.join("config.toml");
        if toml.exists() {
            match Config::load(&toml) {
                Ok(legacy) => {
                    let cfg = legacy.into_runtime();
                    let json = dir.join("config.json");
                    if let Err(e) = cfg.save(&json) {
                        tracing::warn!("failed to seed config.json: {e}");
                    } else {
                        tracing::info!("migrated {} → {}", toml.display(), json.display());
                    }
                    let needs_setup = !cfg.is_configured();
                    return (cfg, dir.clone(), needs_setup);
                }
                Err(e) => {
                    tracing::warn!("ignoring unparseable config.toml at {}: {e}", toml.display())
                }
            }
        }
    }

    let dir = candidate_dirs()
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("."));
    tracing::info!("no config found — starting with defaults (first-run setup)");
    (RuntimeConfig::default(), dir, true)
}

/// Spawns the background proxy server task for the current config.
pub fn spawn_proxy(state: Arc<AppState>) -> JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = proxy_core::serve(state).await {
            tracing::error!("proxy server stopped: {e}");
        }
    })
}

/// Aborts the running proxy task and respawns it (e.g. after a listen-address
/// change), so the new address takes effect without restarting the app.
pub fn restart_proxy(app: &AppHandle, state: Arc<AppState>) {
    let task = app.state::<ProxyTask>();
    let mut guard = task.0.lock().unwrap();
    if let Some(handle) = guard.take() {
        handle.abort();
    }
    *guard = Some(spawn_proxy(state));
}

fn ui_state_path(dir: &Path) -> PathBuf {
    dir.join("ui_state.json")
}

fn config_json_path(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let (config, dir, needs_setup) = resolve_config();

    let state = Arc::new(AppState::new(config));
    state.set_config_path(config_json_path(&dir));
    // Load persisted UI preferences (tray-visible models for this endpoint).
    state.load_ui_state(ui_state_path(&dir));
    let proxy_state = state.clone();

    if let Err(e) = tauri::Builder::default()
        .manage(state)
        .manage(ProxyTask(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::set_api_key,
            commands::forget_api_key,
            commands::set_model,
            commands::run_agent,
            commands::list_agents,
            commands::refresh_models,
            commands::set_visible_models,
            commands::set_endpoint,
            commands::set_listen_addr
        ])
        .setup(move |app| {
            // Start the proxy server in the background and keep its handle so the
            // listen-address command can restart it.
            let handle = spawn_proxy(proxy_state.clone());
            *app.state::<ProxyTask>().0.lock().unwrap() = Some(handle);

            tray::build_tray(app)?;

            // First run (or an unconfigured endpoint): open settings so the user
            // can set the endpoint instead of facing a silent, idle tray icon.
            if needs_setup {
                if let Some(window) = app.get_webview_window("settings") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the settings window only hides it — the app stays in the tray.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
    {
        show_startup_error("Copilot Proxy", &format!("Failed to start the application:\n\n{e}"));
        std::process::exit(1);
    }
}
