use proxy_core::AppState;
use serde::Serialize;
use std::sync::Arc;
use tauri::State;

/// Widok stanu przekazywany do UI (bez ujawniania samego klucza API).
#[derive(Serialize)]
pub struct StateView {
    pub models: Vec<String>,
    pub selected_model: String,
    pub has_api_key: bool,
    pub listen_addr: String,
    pub corporate_base_url: String,
}

#[tauri::command]
pub fn get_state(state: State<'_, Arc<AppState>>) -> StateView {
    StateView {
        models: state.config.models.clone(),
        selected_model: state.selected_model(),
        has_api_key: state.has_api_key(),
        listen_addr: state.config.listen_addr.clone(),
        corporate_base_url: state.config.corporate_base_url.clone(),
    }
}

#[tauri::command]
pub fn set_api_key(state: State<'_, Arc<AppState>>, key: String) {
    state.set_api_key(key);
}

#[tauri::command]
pub fn set_model(state: State<'_, Arc<AppState>>, model: String) -> Result<(), String> {
    if state.set_selected_model(model.clone()) {
        Ok(())
    } else {
        Err(format!("nieznany model: {model}"))
    }
}
