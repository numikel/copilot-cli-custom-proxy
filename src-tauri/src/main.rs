// On Windows in release mode hides the console — the app lives in the tray.
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod commands;
mod config_resolve;
mod lifecycle;
mod startup_error;
mod tray;

use config_resolve::{config_json_path, resolve_config, ui_state_path, ResolvedConfig};
use lifecycle::ProxyTask;
use proxy_core::AppState;
use startup_error::show_startup_error;
use std::sync::Arc;
use tauri::Manager;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let ResolvedConfig {
        config,
        dir,
        needs_setup,
        startup_warning,
    } = resolve_config();

    let state = Arc::new(AppState::new(config));
    state.set_config_path(config_json_path(&dir));
    // Load persisted UI preferences (tray-visible models for this endpoint).
    state.load_ui_state(ui_state_path(&dir));
    let proxy_state = state.clone();

    if let Err(e) = tauri::Builder::default()
        .manage(state)
        .manage(ProxyTask::new())
        .manage(commands::AgentWatch::default())
        .manage(commands::StartupNotice(startup_warning))
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::get_startup_warning,
            commands::set_api_key,
            commands::forget_api_key,
            commands::set_model,
            commands::set_token_limits,
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
            // Bind the listener synchronously so a bind failure surfaces as a
            // startup error, then hand the std socket to the proxy task — which
            // converts it to a Tokio listener inside the runtime (see
            // `ProxyTask::spawn`). Kept as a managed handle so set_listen_addr
            // can restart it.
            let addr = proxy_state.listen_addr();
            let std_listener = std::net::TcpListener::bind(&addr)
                .map_err(|e| format!("cannot bind proxy to {addr}: {e}"))?;
            std_listener.set_nonblocking(true)?;
            app.state::<ProxyTask>()
                .spawn(std_listener, proxy_state.clone());

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
