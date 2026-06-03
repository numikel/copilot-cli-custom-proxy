// Na Windows w trybie release ukrywa konsolę — aplikacja żyje w trayu.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod commands;
mod tray;

use proxy_core::{AppState, Config};
use std::path::PathBuf;
use std::sync::Arc;

/// Lokalizacje, w których szukamy `config.toml`: obok pliku wykonywalnego,
/// a następnie w bieżącym katalogu roboczym.
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
                tracing::info!("wczytano konfigurację z {}", path.display());
                return config;
            }
            Err(e) => panic!("{} ({})", e, path.display()),
        }
    }
    panic!(
        "nie znaleziono config.toml — skopiuj config.example.toml do config.toml. Sprawdzone ścieżki: {:?}",
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
    let tray_state = state.clone();

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::set_api_key,
            commands::set_model
        ])
        .setup(move |app| {
            // Serwer proxy startuje w tle, na adresie z konfiguracji.
            let serve_state = proxy_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = proxy_core::serve(serve_state).await {
                    tracing::error!("serwer proxy zakończył działanie: {e}");
                }
            });
            tray::build_tray(app, tray_state.clone())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Zamknięcie okna ustawień tylko je chowa — aplikacja zostaje w trayu.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("błąd uruchomienia aplikacji Tauri");
}
