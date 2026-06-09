// On Windows in release mode hides the console — the app lives in the tray.
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod commands;
mod startup_error;
mod tray;

use proxy_core::{AppState, Config, RuntimeConfig};
use startup_error::show_startup_error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{async_runtime::JoinHandle, Manager};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// Handle to the background proxy task, kept in Tauri-managed state so the
/// listen-address command can restart it. Holds both the task handle and a
/// shutdown sender so the server can be stopped *gracefully* — the old server
/// releases its port before a replacement binds. Lives in the GUI crate to keep
/// `proxy-core` free of Tauri dependencies.
pub struct ProxyTask {
    handle: Mutex<Option<JoinHandle<()>>>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
}

impl ProxyTask {
    fn new() -> Self {
        Self {
            handle: Mutex::new(None),
            shutdown: Mutex::new(None),
        }
    }

    /// Spawns the proxy server on an already-bound listener, wiring up a
    /// graceful-shutdown channel. Replaces any previously stored handle/sender
    /// (call [`ProxyTask::stop`] first to tear the old one down cleanly).
    pub fn spawn(&self, listener: TcpListener, state: Arc<AppState>) {
        let (tx, rx) = oneshot::channel::<()>();
        let handle = tauri::async_runtime::spawn(async move {
            // Resolves when the sender fires *or* is dropped — either way the
            // server shuts down.
            let shutdown = async {
                let _ = rx.await;
            };
            if let Err(e) = proxy_core::serve_with(listener, state, shutdown).await {
                tracing::error!("proxy server stopped: {e}");
            }
        });
        *self.handle.lock().unwrap() = Some(handle);
        *self.shutdown.lock().unwrap() = Some(tx);
    }

    /// Signals the running server to shut down and waits for the task to finish,
    /// guaranteeing the listening socket is released before returning. Locks are
    /// released before the `.await` so no `MutexGuard` is held across it.
    pub async fn stop(&self) {
        let tx = self.shutdown.lock().unwrap().take();
        drop(tx); // dropping (or sending on) the channel triggers shutdown
        let handle = self.handle.lock().unwrap().take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }
}

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

/// Re-validates a config loaded from disk and falls back to safe values for any
/// field that fails. `config.json` can be hand-edited (or swapped out), so a
/// malformed `listen_addr` must never reach the agent launcher (command
/// injection) and a malformed `endpoint_url` must not silently misroute or leak
/// credentials. Invalid `listen_addr` → loopback default; invalid
/// `endpoint_url` → cleared (forces the settings window on first run).
fn sanitize_config(mut cfg: RuntimeConfig) -> RuntimeConfig {
    if let Err(e) = proxy_core::validate_listen_addr(&cfg.listen_addr) {
        tracing::warn!(
            addr = %cfg.listen_addr,
            "invalid listen_addr in config ({e}) — falling back to {}",
            proxy_core::DEFAULT_LISTEN_ADDR
        );
        cfg.listen_addr = proxy_core::DEFAULT_LISTEN_ADDR.to_string();
    }
    // A non-loopback bind must never come up without the explicit exposure
    // opt-in (which gates the gateway token) — a hand-edited config that sets a
    // LAN address but not the flag is reset to loopback.
    if !proxy_core::is_loopback_listen_addr(&cfg.listen_addr) && !cfg.expose_to_network {
        tracing::warn!(
            addr = %cfg.listen_addr,
            "non-loopback listen_addr without expose_to_network — falling back to {}",
            proxy_core::DEFAULT_LISTEN_ADDR
        );
        cfg.listen_addr = proxy_core::DEFAULT_LISTEN_ADDR.to_string();
    }
    // An exposed proxy is never tokenless: mint one if the config enabled
    // exposure but carries no token (e.g. hand-edited).
    if cfg.expose_to_network && cfg.proxy_token.as_deref().unwrap_or("").is_empty() {
        cfg.proxy_token = Some(proxy_core::generate_proxy_token());
    }
    if !cfg.endpoint_url.is_empty() {
        if let Err(e) = proxy_core::validate_endpoint_url(&cfg.endpoint_url) {
            tracing::warn!(
                url = %cfg.endpoint_url,
                "invalid endpoint_url in config ({e}) — clearing it"
            );
            cfg.endpoint_url = String::new();
        }
    }
    cfg
}

/// Resolves the runtime config at startup:
/// 1. an existing `config.json` (the source of truth), else
/// 2. a legacy `config.toml`, migrated and seeded into `config.json`, else
/// 3. built-in defaults.
///
/// Loaded values are sanitized (see [`sanitize_config`]) before use.
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
                let cfg = sanitize_config(cfg);
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
                    let cfg = sanitize_config(legacy.into_runtime());
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
                    tracing::warn!(
                        "ignoring unparseable config.toml at {}: {e}",
                        toml.display()
                    )
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
        .manage(ProxyTask::new())
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
            commands::set_listen_addr,
            commands::set_expose_to_network,
            commands::regenerate_proxy_token
        ])
        .setup(move |app| {
            // Bind the listener synchronously (no async runtime needed yet) so a
            // bind failure surfaces as a startup error, then hand it to the proxy
            // task. Kept as a managed handle so set_listen_addr can restart it.
            let addr = proxy_state.listen_addr();
            let std_listener = std::net::TcpListener::bind(&addr)
                .map_err(|e| format!("cannot bind proxy to {addr}: {e}"))?;
            std_listener.set_nonblocking(true)?;
            let listener = TcpListener::from_std(std_listener)?;
            app.state::<ProxyTask>()
                .spawn(listener, proxy_state.clone());

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
        show_startup_error(
            "Copilot Proxy",
            &format!("Failed to start the application:\n\n{e}"),
        );
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ProxyTask::stop` must wait for the server to fully shut down so the
    /// listening socket is released — otherwise a restart on the same address
    /// races into `EADDRINUSE`. We run on Tauri's async runtime (the same one
    /// `spawn` uses) so the listener and the server task share a runtime.
    #[test]
    fn proxy_task_stop_releases_port() {
        tauri::async_runtime::block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();

            let state = Arc::new(AppState::new(RuntimeConfig::default()));
            let task = ProxyTask::new();
            task.spawn(listener, state);

            // Graceful stop must release the socket before returning.
            task.stop().await;

            std::net::TcpListener::bind(addr).expect("port should be free after ProxyTask::stop()");
        });
    }
}
