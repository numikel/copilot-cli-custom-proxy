use proxy_core::AppState;
use std::sync::Arc;
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    AppHandle, Manager, Wry,
};

const TRAY_ID: &str = "main";

/// Models shown in the tray's "Models" submenu: the endpoint's curated visible
/// set (chosen in the settings window; defaults to all chat models), with the
/// active model pinned first so its checkmark is always reachable.
fn tray_submenu_ids(state: &AppState, selected: &str) -> Vec<String> {
    let mut ids = state.visible_model_ids();
    if !selected.is_empty() && !ids.iter().any(|id| id == selected) {
        ids.insert(0, selected.to_string());
    }
    ids
}

/// True when the proxy is ready to serve: a key is set and a model is selected.
/// Drives the active (accent) vs idle (muted) tray icon.
fn is_active(state: &AppState) -> bool {
    state.has_api_key() && !state.selected_model().is_empty()
}

/// Swaps the tray icon between the active and idle artwork for the given state.
fn update_tray_icon(app: &AppHandle, active: bool) {
    const ACTIVE: &[u8] = include_bytes!("../icons/tray-active.png");
    const IDLE: &[u8] = include_bytes!("../icons/tray-idle.png");
    let bytes = if active { ACTIVE } else { IDLE };
    if let (Some(tray), Ok(image)) = (app.tray_by_id(TRAY_ID), Image::from_bytes(bytes)) {
        let _ = tray.set_icon(Some(image));
    }
}

fn status_text(state: &AppState) -> String {
    if state.active_api().is_none() {
        return "Not configured — open Settings to set the endpoint".to_string();
    }
    let models = state.models();
    if models.is_empty() {
        "No models — set API key, then Refresh models".to_string()
    } else {
        format!("Active model: {}", state.selected_model())
    }
}

/// Builds the tray menu from the current state: status line, model toggles,
/// then Refresh models / Run <agent> (one per supported agent) / Settings / Quit.
fn build_menu(app: &AppHandle, state: &AppState) -> tauri::Result<Menu<Wry>> {
    let selected = state.selected_model();

    let status = MenuItem::with_id(app, "status", status_text(state), false, None::<&str>)?;

    let menu = Menu::new(app)?;
    menu.append(&status)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    // Models live in their own submenu so the first level stays short and
    // "Open Settings…"/"Quit" are always reachable, even with hundreds of
    // models. The submenu shows the endpoint's curated chat models (see the
    // settings window); the full catalog is always available there.
    let models_menu = Submenu::with_id(app, "models_menu", "Models", true)?;
    let ids = tray_submenu_ids(state, &selected);
    if ids.is_empty() {
        let empty = MenuItem::with_id(
            app,
            "models_empty",
            "No models — Refresh first",
            false,
            None::<&str>,
        )?;
        models_menu.append(&empty)?;
    } else {
        for model in ids {
            let checked = model == selected;
            let item = CheckMenuItem::with_id(
                app,
                format!("model::{model}"),
                &model,
                true,
                checked,
                None::<&str>,
            )?;
            models_menu.append(&item)?;
        }
    }
    menu.append(&models_menu)?;

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        "refresh_models",
        "Refresh models",
        true,
        None::<&str>,
    )?)?;
    // Only offer agents the configured upstream can actually serve.
    for &agent in crate::commands::Agent::ALL {
        if crate::commands::agent_supported(state, agent) {
            menu.append(&MenuItem::with_id(
                app,
                format!("run::{}", agent.id()),
                format!("Run {}", agent.label()),
                true,
                None::<&str>,
            )?)?;
        }
    }
    menu.append(&MenuItem::with_id(
        app,
        "settings",
        "Open Settings…",
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?)?;

    Ok(menu)
}

/// Rebuilds the tray menu from the current state and applies it. Call this
/// whenever the model list or selection changes.
pub fn apply_menu(app: &AppHandle) -> tauri::Result<()> {
    let state = app.state::<Arc<AppState>>();
    let menu = build_menu(app, &state)?;
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        tray.set_menu(Some(menu))?;
    }
    update_tray_icon(app, is_active(&state));
    Ok(())
}

/// Creates the tray icon. The menu event handler reads managed state and
/// rebuilds the menu via [`apply_menu`], so it always reflects current models.
pub fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let state = app.state::<Arc<AppState>>();
    let menu = build_menu(app.handle(), &state)?;

    let icon = app
        .default_window_icon()
        .cloned()
        .expect("missing default app icon (check bundle.icon in tauri.conf.json)");

    let _tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("Copilot Proxy")
        .menu(&menu)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref().to_string();
            if id == "quit" {
                app.exit(0);
            } else if let Some(agent_id) = id.strip_prefix("run::") {
                if let Some(kind) = crate::commands::Agent::from_id(agent_id) {
                    let state = app.state::<Arc<AppState>>();
                    if let Err(e) = crate::commands::launch_agent(&state, kind) {
                        tracing::error!("failed to launch {agent_id}: {e}");
                    }
                }
            } else if id == "settings" {
                if let Some(window) = app.get_webview_window("settings") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            } else if id == "refresh_models" {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<Arc<AppState>>().inner().clone();
                    match proxy_core::fetch_models(&state).await {
                        Ok(models) => {
                            state.set_models(models);
                            let _ = apply_menu(&app);
                        }
                        Err(e) => tracing::error!("refresh models failed: {e}"),
                    }
                });
            } else if let Some(model) = id.strip_prefix("model::") {
                let state = app.state::<Arc<AppState>>();
                state.set_selected_model(model.to_string());
                let _ = apply_menu(app);
            }
        })
        .build(app)?;

    // Reflect the initial readiness in the tray icon (idle until a key + model).
    update_tray_icon(app.handle(), is_active(&state));

    Ok(())
}
