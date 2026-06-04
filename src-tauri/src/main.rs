// On Windows in release mode hides the console — the app lives in the tray.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod commands;
mod startup_error;
mod tray;

use proxy_core::{AppState, Config};
use startup_error::{exit_with_startup_error, show_startup_error, ConfigLoadError};
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

fn load_config() -> Result<(Config, PathBuf), ConfigLoadError> {
    let candidates = config_candidates();
    for path in &candidates {
        if !path.exists() {
            continue;
        }
        let config = Config::load(path).map_err(|e| ConfigLoadError::Invalid {
            path: path.clone(),
            message: e.to_string(),
        })?;
        tracing::info!("loaded configuration from {}", path.display());
        return Ok((config, path.clone()));
    }
    Err(ConfigLoadError::NotFound { candidates })
}

/// UI preferences live next to `config.toml` (or the cwd as a fallback).
fn ui_state_path(config_path: &std::path::Path) -> PathBuf {
    config_path
        .parent()
        .map(|dir| dir.join("ui_state.json"))
        .unwrap_or_else(|| PathBuf::from("ui_state.json"))
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let (config, config_path) = match load_config() {
        Ok(loaded) => loaded,
        Err(err) => exit_with_startup_error(err),
    };

    let state = Arc::new(AppState::new(config));
    // Load persisted UI preferences (tray-visible models for this endpoint).
    state.load_ui_state(ui_state_path(&config_path));
    let proxy_state = state.clone();

    if let Err(e) = tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::set_api_key,
            commands::forget_api_key,
            commands::set_model,
            commands::run_agent,
            commands::list_agents,
            commands::refresh_models,
            commands::set_visible_models
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
    {
        show_startup_error("Copilot Proxy", &format!("Failed to start the application:\n\n{e}"));
        std::process::exit(1);
    }
}
