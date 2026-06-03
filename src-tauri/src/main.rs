// On Windows in release mode hides the console — the app lives in the tray.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod commands;
mod tray;

use proxy_core::{AppState, Config};
use std::path::PathBuf;
use std::sync::Arc;

/// Locations searched for `config.toml`: next to the executable,
/// then in the current working directory.
fn config_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("config.toml"));
        }
    }
    paths.push(PathBuf::from("config.toml"));
    paths
}

fn load_config() -> Config {
    let candidates = config_candidates();
    for path in &candidates {
        if !path.exists() {
            continue;
        }
        match Config::load(path) {
            Ok(config) => {
                tracing::info!("loaded configuration from {}", path.display());
                return config;
            }
            Err(e) => panic!("{} ({})", e, path.display()),
        }
    }
    panic!(
        "config.toml not found — copy config.example.toml to config.toml. Checked paths: {:?}",
        candidates
    );
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = load_config();
    let state = Arc::new(AppState::new(config));
    let proxy_state = state.clone();

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::set_api_key,
            commands::set_model,
            commands::run_agent,
            commands::refresh_models
        ])
        .setup(move |app| {
            // The proxy server starts in the background, on the address from the config.
            let serve_state = proxy_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = proxy_core::serve(serve_state).await {
                    tracing::error!("proxy server stopped: {e}");
                }
            });
            tray::build_tray(app)?;
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
        .expect("failed to run the Tauri application");
}
