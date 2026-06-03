use proxy_core::AppState;
use std::sync::Arc;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, Wry,
};

const TRAY_ID: &str = "main";

fn status_text(state: &AppState) -> String {
    let models = state.models();
    if models.is_empty() {
        "No models — set API key, then Refresh models".to_string()
    } else {
        format!("Active model: {}", state.selected_model())
    }
}

/// Builds the tray menu from the current state: status line, model toggles,
/// then Refresh models / Run Copilot / Set API key / Quit.
fn build_menu(app: &AppHandle, state: &AppState) -> tauri::Result<Menu<Wry>> {
    let selected = state.selected_model();

    let status = MenuItem::with_id(app, "status", status_text(state), false, None::<&str>)?;

    let menu = Menu::new(app)?;
    menu.append(&status)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    for model in state.models() {
        let item = CheckMenuItem::with_id(
            app,
            format!("model::{model}"),
            &model,
            true,
            model == selected,
            None::<&str>,
        )?;
        menu.append(&item)?;
    }

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "refresh_models", "Refresh models", true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "run_copilot", "Run Copilot", true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "settings", "Open Settings…", true, None::<&str>)?)?;
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
            } else if id == "run_copilot" {
                let state = app.state::<Arc<AppState>>();
                if let Err(e) = crate::commands::launch_copilot(&state) {
                    tracing::error!("failed to launch Copilot: {e}");
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

    Ok(())
}
