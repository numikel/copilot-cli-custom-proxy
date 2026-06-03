use proxy_core::AppState;
use std::sync::Arc;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    App, Manager,
};

fn status_text(model: &str) -> String {
    format!("Active model: {model}")
}

/// Builds the tray icon with a menu: status, model list (toggles),
/// open settings, and quit.
pub fn build_tray(app: &App, state: Arc<AppState>) -> tauri::Result<()> {
    let handle = app.handle();
    let selected = state.selected_model();

    let status = MenuItem::with_id(
        handle,
        "status",
        status_text(&selected),
        false,
        None::<&str>,
    )?;

    let mut model_items = Vec::new();
    for model in &state.config.models {
        let item = CheckMenuItem::with_id(
            handle,
            format!("model::{model}"),
            model,
            true,
            model == &selected,
            None::<&str>,
        )?;
        model_items.push(item);
    }

    let run_copilot = MenuItem::with_id(handle, "run_copilot", "Run Copilot", true, None::<&str>)?;
    let settings = MenuItem::with_id(handle, "settings", "Set API key…", true, None::<&str>)?;
    let quit = MenuItem::with_id(handle, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::new(handle)?;
    menu.append(&status)?;
    menu.append(&PredefinedMenuItem::separator(handle)?)?;
    for item in &model_items {
        menu.append(item)?;
    }
    menu.append(&PredefinedMenuItem::separator(handle)?)?;
    menu.append(&run_copilot)?;
    menu.append(&settings)?;
    menu.append(&quit)?;

    let icon = app
        .default_window_icon()
        .cloned()
        .expect("missing default app icon (check bundle.icon in tauri.conf.json)");

    let model_items_handler = model_items.clone();
    let status_handler = status.clone();
    let state_handler = state.clone();

    let _tray = TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("Copilot Proxy")
        .menu(&menu)
        .on_menu_event(move |app, event| {
            let id = event.id().as_ref().to_string();
            if id == "quit" {
                app.exit(0);
            } else if id == "run_copilot" {
                if let Err(e) = crate::commands::launch_copilot(&state_handler) {
                    tracing::error!("failed to launch Copilot: {e}");
                }
            } else if id == "settings" {
                if let Some(window) = app.get_webview_window("settings") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            } else if let Some(model) = id.strip_prefix("model::") {
                state_handler.set_selected_model(model.to_string());
                for item in &model_items_handler {
                    let _ = item.set_checked(item.id().as_ref() == id);
                }
                let _ = status_handler.set_text(status_text(model));
            }
        })
        .build(app)?;

    Ok(())
}
