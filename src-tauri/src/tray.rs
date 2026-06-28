use proxy_core::{ApiKind, AppState, CcSlot, CcSlots};
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

/// Pure status-line text, factored out so every branch is unit-testable without
/// constructing an `AppState`. `chat_ids` is the chat-only catalog (what the
/// "Models" submenu offers); `selected` is the active model id.
fn status_line(api_configured: bool, chat_ids: &[String], selected: &str) -> String {
    if !api_configured {
        return "Not configured — open Settings to set the endpoint".to_string();
    }
    if chat_ids.is_empty() {
        return "No models — set API key, then Refresh models".to_string();
    }
    if selected.is_empty() {
        return "No model selected — pick one from Models".to_string();
    }
    format!("Active model: {selected}")
}

fn status_text(state: &AppState) -> String {
    if state.active_api() == Some(ApiKind::Messages) {
        return cc_status_line(&state.cc_slots());
    }
    status_line(
        state.active_api().is_some(),
        &state.chat_model_ids(),
        &state.selected_model(),
    )
}

/// Status-line text for a Claude Code (messages) endpoint, factored out so it is
/// unit-testable without an `AppState`.
fn cc_status_line(slots: &CcSlots) -> String {
    if slots.is_complete() {
        "Claude Code — all slots set".to_string()
    } else {
        format!("Claude Code — {}/5 slots set", slots.configured_count())
    }
}

/// Appends the model-selection section. Chat/responses get a single "Models"
/// submenu (the active model checked). The messages API gets one submenu per
/// Claude Code slot, each listing the endpoint's visible chat models with the
/// slot's current choice checked; the subagent submenu adds an "Inherit" toggle.
/// Menu item ids: `model::<id>` (single) and `cc::<slot>::<id>` /
/// `cc::subagent::__inherit__` (slots).
fn append_models_section(
    app: &AppHandle,
    menu: &Menu<Wry>,
    state: &AppState,
) -> tauri::Result<()> {
    if state.active_api() == Some(ApiKind::Messages) {
        let slots = state.cc_slots();
        let visible = state.visible_model_ids();
        for &slot in CcSlot::ALL {
            let submenu = Submenu::with_id(app, format!("cc_menu::{}", slot.id()), slot.display_name(), true)?;
            if slot == CcSlot::Subagent {
                let inherit = CheckMenuItem::with_id(
                    app,
                    "cc::subagent::__inherit__",
                    "Inherit",
                    true,
                    slots.subagent_inherit,
                    None::<&str>,
                )?;
                submenu.append(&inherit)?;
                submenu.append(&PredefinedMenuItem::separator(app)?)?;
            }
            if visible.is_empty() {
                let empty = MenuItem::with_id(
                    app,
                    format!("cc_empty::{}", slot.id()),
                    "No models — Refresh first",
                    false,
                    None::<&str>,
                )?;
                submenu.append(&empty)?;
            } else {
                let current = slots.model_for(slot);
                for model in &visible {
                    let item = CheckMenuItem::with_id(
                        app,
                        format!("cc::{}::{}", slot.id(), model),
                        model,
                        true,
                        current == Some(model.as_str()),
                        None::<&str>,
                    )?;
                    submenu.append(&item)?;
                }
            }
            menu.append(&submenu)?;
        }
        return Ok(());
    }

    // Chat / responses: a single flat Models submenu (existing behavior).
    let selected = state.selected_model();
    let models_menu = Submenu::with_id(app, "models_menu", "Models", true)?;
    let ids = tray_submenu_ids(state, &selected);
    if ids.is_empty() {
        let empty = MenuItem::with_id(app, "models_empty", "No models — Refresh first", false, None::<&str>)?;
        models_menu.append(&empty)?;
    } else {
        for model in ids {
            let checked = model == selected;
            let item = CheckMenuItem::with_id(app, format!("model::{model}"), &model, true, checked, None::<&str>)?;
            models_menu.append(&item)?;
        }
    }
    menu.append(&models_menu)?;
    Ok(())
}

/// Builds the tray menu from the current state: status line, model toggles,
/// then Refresh models / Run <agent> (one per supported agent) / Settings / Quit.
fn build_menu(app: &AppHandle, state: &AppState) -> tauri::Result<Menu<Wry>> {
    let status = MenuItem::with_id(app, "status", status_text(state), false, None::<&str>)?;

    let menu = Menu::new(app)?;
    menu.append(&status)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    append_models_section(app, &menu, state)?;

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
            // Claude Code needs all slots configured before it can launch.
            if agent == crate::commands::Agent::ClaudeCode && !state.cc_slots_complete() {
                continue;
            }
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
                    if let Err(e) = crate::commands::launch_agent(app, kind) {
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
            } else if let Some(rest) = id.strip_prefix("cc::") {
                // rest = "<slot>::<value>" where value is a model id or "__inherit__".
                if let Some((slot_id, value)) = rest.split_once("::") {
                    if let Some(slot) = crate::commands::cc_slot_from_id(slot_id) {
                        let state = app.state::<Arc<AppState>>().inner().clone();
                        let result = if value == "__inherit__" {
                            state.set_cc_slot(slot, None, true)
                        } else {
                            state.set_cc_slot(slot, Some(value.to_string()), false)
                        };
                        if let Err(e) = result {
                            tracing::error!("set cc slot failed: {e}");
                        }
                        let _ = apply_menu(app);
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::status_line;
    use proxy_core::{CcSlot, CcSlots};

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn cc_status_counts_configured_slots() {
        let mut slots = CcSlots::default();
        assert_eq!(super::cc_status_line(&slots), "Claude Code — 0/5 slots set");
        slots.set(CcSlot::Opus, Some("a".into()), false);
        slots.set(CcSlot::Sonnet, Some("b".into()), false);
        slots.set(CcSlot::Haiku, Some("c".into()), false);
        slots.set(CcSlot::Fable, Some("d".into()), false);
        assert_eq!(super::cc_status_line(&slots), "Claude Code — 4/5 slots set");
        slots.set(CcSlot::Subagent, None, true);
        assert_eq!(super::cc_status_line(&slots), "Claude Code — all slots set");
    }

    #[test]
    fn status_unconfigured() {
        assert_eq!(
            status_line(false, &[], ""),
            "Not configured — open Settings to set the endpoint"
        );
    }

    /// The unconfigured check wins even if a catalog/selection somehow lingers.
    #[test]
    fn status_unconfigured_gates_first() {
        assert_eq!(
            status_line(false, &ids(&["gpt"]), "gpt"),
            "Not configured — open Settings to set the endpoint"
        );
    }

    #[test]
    fn status_no_chat_models() {
        assert_eq!(
            status_line(true, &[], ""),
            "No models — set API key, then Refresh models"
        );
    }

    /// Configured with chat models available but none selected — the branch that
    /// replaces the old empty "Active model: ".
    #[test]
    fn status_no_selection() {
        assert_eq!(
            status_line(true, &ids(&["gpt-4"]), ""),
            "No model selected — pick one from Models"
        );
    }

    #[test]
    fn status_active() {
        assert_eq!(
            status_line(true, &ids(&["gpt-4"]), "gpt-4"),
            "Active model: gpt-4"
        );
    }
}
