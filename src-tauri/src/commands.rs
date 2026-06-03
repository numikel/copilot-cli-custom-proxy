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

/// The model identifier passed to the launched CLI. Its value is arbitrary —
/// the proxy rewrites the `model` field on every request — so we use a friendly
/// label that makes it obvious traffic flows through this switcher.
/// Only read by the Windows launcher; harmless elsewhere.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const PROXY_MODEL_LABEL: &str = "copilot-proxy-model";

/// CLI agents the launcher knows how to start against the proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Copilot,
    Codex,
}

impl Agent {
    /// Parses the agent id sent from the UI / tray (e.g. "copilot", "codex").
    pub fn from_id(id: &str) -> Option<Agent> {
        match id {
            "copilot" => Some(Agent::Copilot),
            "codex" => Some(Agent::Codex),
            _ => None,
        }
    }
}

#[tauri::command]
pub fn run_agent(state: State<'_, Arc<AppState>>, agent: String) -> Result<(), String> {
    let kind = Agent::from_id(&agent).ok_or_else(|| format!("unknown agent: {agent}"))?;
    launch_agent(&state, kind)
}

/// Opens a new terminal with the proxy environment set and starts the selected
/// agent pointed at the proxy. Shared by the tray menu and the settings window.
pub fn launch_agent(state: &AppState, kind: Agent) -> Result<(), String> {
    let base_url = format!("http://{}", state.config.listen_addr);
    spawn_agent(kind, &base_url).map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn spawn_agent(kind: Agent, base_url: &str) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    // CREATE_NEW_CONSOLE — give the spawned PowerShell its own visible window.
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    let mut command = std::process::Command::new("powershell");
    command.creation_flags(CREATE_NEW_CONSOLE);

    match kind {
        Agent::Copilot => {
            // Copilot reads the endpoint and model straight from the environment.
            command
                .args(["-NoExit", "-Command", "copilot"])
                .env("COPILOT_PROVIDER_BASE_URL", base_url)
                .env("COPILOT_MODEL", PROXY_MODEL_LABEL);
        }
        Agent::Codex => {
            // Codex only speaks the Responses API (the "chat" wire API was
            // removed in Feb 2026), so the upstream behind the proxy must
            // support /responses. We define an ephemeral provider via `-c`
            // overrides instead of editing the user's ~/.codex/config.toml.
            // The env_key must point at a set variable; the value is a dummy
            // because the proxy injects the real key from memory.
            command
                .args(["-NoExit", "-Command", &codex_command(base_url)])
                .env(CODEX_KEY_ENV, "proxy-managed");
        }
    }

    command.spawn()?;
    Ok(())
}

/// Builds the `codex` invocation that points an ephemeral provider at the proxy.
/// Values contain no spaces, so no shell quoting is required.
#[cfg(target_os = "windows")]
fn codex_command(base_url: &str) -> String {
    format!(
        "codex \
         -c model_provider=proxy \
         -c model_providers.proxy.name=copilot-proxy \
         -c model_providers.proxy.base_url={base_url} \
         -c model_providers.proxy.wire_api=responses \
         -c model_providers.proxy.env_key={CODEX_KEY_ENV} \
         -c model={PROXY_MODEL_LABEL}"
    )
}

/// Environment variable Codex reads the (dummy) API key from.
#[cfg(target_os = "windows")]
const CODEX_KEY_ENV: &str = "CODEX_PROXY_KEY";

#[cfg(not(target_os = "windows"))]
fn spawn_agent(_kind: Agent, _base_url: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Launching an agent is only supported on Windows",
    ))
}
