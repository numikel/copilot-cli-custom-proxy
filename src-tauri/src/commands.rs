use proxy_core::{AppState, RequestLog};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

/// State view passed to the UI (without exposing the API key itself).
#[derive(Serialize)]
pub struct StateView {
    pub models: Vec<String>,
    pub selected_model: String,
    pub has_api_key: bool,
    pub listen_addr: String,
    pub corporate_base_url: String,
    /// Live snapshot of forwarded traffic, so the UI can show what Copilot hits.
    pub request_log: RequestLog,
}

#[tauri::command]
pub fn get_state(state: State<'_, Arc<AppState>>) -> StateView {
    StateView {
        models: state.models(),
        selected_model: state.selected_model(),
        has_api_key: state.has_api_key(),
        listen_addr: state.config.listen_addr.clone(),
        corporate_base_url: state.config.corporate_base_url.clone(),
        request_log: state.request_log(),
    }
}

#[tauri::command]
pub fn set_api_key(state: State<'_, Arc<AppState>>, key: String) {
    state.set_api_key(key);
}

/// Fetches the model list from `{corporate_base_url}/models`, stores it, and
/// rebuilds the tray menu. Returns the fetched models for the UI.
#[tauri::command]
pub async fn refresh_models(app: AppHandle) -> Result<Vec<String>, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let models = proxy_core::fetch_models(&state).await?;
    state.set_models(models.clone());
    let _ = crate::tray::apply_menu(&app);
    Ok(models)
}

#[tauri::command]
pub fn set_model(state: State<'_, Arc<AppState>>, model: String) -> Result<(), String> {
    if state.set_selected_model(model.clone()) {
        Ok(())
    } else {
        Err(format!("unknown model: {model}"))
    }
}

#[tauri::command]
pub fn run_copilot(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    launch_copilot(&state)
}

/// Opens a new terminal with the proxy environment variables set and runs
/// `copilot`. Shared by the tray menu and the settings window button.
pub fn launch_copilot(state: &AppState) -> Result<(), String> {
    let base_url = format!("http://{}", state.config.listen_addr);
    spawn_copilot(&base_url).map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn spawn_copilot(base_url: &str) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    // CREATE_NEW_CONSOLE — give the spawned PowerShell its own visible window.
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    // The COPILOT_MODEL value is arbitrary — the proxy rewrites the model on
    // every request — so we use a friendly label that makes it obvious in
    // Copilot's UI that traffic flows through this switcher.
    const COPILOT_MODEL_LABEL: &str = "copilot-proxy-model";
    std::process::Command::new("powershell")
        .args(["-NoExit", "-Command", "copilot"])
        .env("COPILOT_PROVIDER_BASE_URL", base_url)
        .env("COPILOT_MODEL", COPILOT_MODEL_LABEL)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn spawn_copilot(_base_url: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Run Copilot is only supported on Windows",
    ))
}
