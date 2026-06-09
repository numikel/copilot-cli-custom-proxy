use proxy_core::{AppState, ModelInfo, RequestLog};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

/// State view passed to the UI (without exposing the API key itself).
#[derive(Serialize)]
pub struct StateView {
    /// Available models, each classified as chat / non-chat for filtering.
    pub models: Vec<ModelInfo>,
    pub selected_model: String,
    pub has_api_key: bool,
    /// Local address the proxy listens on (editable in the settings window).
    pub listen_addr: String,
    /// Full upstream endpoint URL, including the API suffix. Empty = not configured.
    pub endpoint_url: String,
    /// Wire API derived from the endpoint URL: "chat", "responses", or null when
    /// the URL is empty / its suffix is unrecognized. Gates launchable agents.
    pub active_api: Option<String>,
    /// Model ids shown in the tray's "Models" submenu for this endpoint
    /// (curated in the settings window; defaults to all chat models).
    pub visible_models: Vec<String>,
    /// Whether the proxy is allowed to bind beyond loopback (network exposure).
    pub expose_to_network: bool,
    /// Gateway token non-loopback clients must present (shown so the user can
    /// copy it to a remote device). `None`/absent until exposure is enabled.
    pub proxy_token: Option<String>,
    /// Live snapshot of forwarded traffic, so the UI can show what Copilot hits.
    pub request_log: RequestLog,
}

/// Builds the JS-facing view from current state.
fn state_view(state: &AppState) -> StateView {
    StateView {
        models: state.models(),
        selected_model: state.selected_model(),
        has_api_key: state.has_api_key(),
        listen_addr: state.listen_addr(),
        endpoint_url: state.endpoint_url(),
        active_api: state.active_api().map(|a| a.as_str().to_string()),
        visible_models: state.visible_model_ids(),
        expose_to_network: state.expose_to_network(),
        proxy_token: state.proxy_token(),
        request_log: state.request_log(),
    }
}

#[tauri::command]
pub fn get_state(state: State<'_, Arc<AppState>>) -> StateView {
    state_view(&state)
}

/// Sets the upstream endpoint URL (must end in /chat/completions or /responses)
/// and refreshes the model catalog. To avoid a window where the new URL is
/// paired with a stale/empty catalog (which would forward `"model": ""`), the
/// new endpoint's models are fetched *before* mutating shared state, then the
/// URL and catalog are swapped atomically. On a fetch error the new URL is
/// still committed with an empty catalog (the user picked it deliberately;
/// Refresh recovers once the network is back). Rebuilds the tray.
#[tauri::command]
pub async fn set_endpoint(app: AppHandle, url: String) -> Result<StateView, String> {
    let url = url.trim().to_string();
    proxy_core::validate_endpoint_url(&url)?;

    let state = app.state::<Arc<AppState>>().inner().clone();

    // Probe the candidate endpoint without touching live state.
    let models = if let Some(key) = state.api_key() {
        match proxy_core::fetch_models_from(&state.http, &url, &key).await {
            Ok(models) => models,
            Err(e) => {
                tracing::warn!("model refresh for new endpoint failed: {e}");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    state.swap_endpoint(url, models);
    let _ = crate::tray::apply_menu(&app);
    Ok(state_view(&state))
}

/// Sets the local listen address (validated as host:port) and restarts the
/// background proxy on the new address. The restart is robust:
/// - a no-op when the address is unchanged (avoids tearing down a working
///   server and a spurious "address in use" against itself);
/// - the new address is **bound eagerly** here, so a bind failure (e.g. port in
///   use) surfaces as an `Err` to the UI while the old server keeps running;
/// - only after a successful bind is the old server gracefully stopped (its
///   port released) and the new one spawned on the already-bound listener.
#[tauri::command]
pub async fn set_listen_addr(app: AppHandle, addr: String) -> Result<StateView, String> {
    let addr = addr.trim().to_string();
    proxy_core::validate_listen_addr(&addr)?;

    let state = app.state::<Arc<AppState>>().inner().clone();

    // A non-loopback bind shares the injected API key with the network, so it is
    // only allowed once the user has opted into exposure (which also mints the
    // gateway token).
    if !proxy_core::is_loopback_listen_addr(&addr) && !state.expose_to_network() {
        return Err(
            "This address is not loopback. Enable \"Expose to network\" \
                    first to bind beyond 127.0.0.1."
                .to_string(),
        );
    }

    if !listen_addr_changed(&state.listen_addr(), &addr) {
        return Ok(state_view(&state));
    }

    // Bind the new address before touching the running server. A different
    // address never collides with the old one; a failure here leaves the old
    // server intact (safe rollback) and is reported to the UI.
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("cannot bind proxy to {addr}: {e}"))?;

    state.set_listen_addr(addr);
    let task = app.state::<crate::ProxyTask>();
    task.stop().await;
    task.spawn(listener, state.clone());

    let _ = crate::tray::apply_menu(&app);
    Ok(state_view(&state))
}

/// Whether a candidate listen address differs from the current one (trimmed
/// comparison). Pulled out so the skip-if-unchanged decision is unit-testable.
pub(crate) fn listen_addr_changed(current: &str, candidate: &str) -> bool {
    current.trim() != candidate.trim()
}

/// Sets which models appear in the tray's "Models" submenu (curated in the
/// settings window), persists the choice per-endpoint, and rebuilds the tray.
#[tauri::command]
pub fn set_visible_models(app: AppHandle, models: Vec<String>) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state.set_visible_models(models);
    let _ = crate::tray::apply_menu(&app);
    Ok(())
}

/// Toggles whether the proxy may bind beyond loopback. Enabling it mints a
/// gateway token (if none exists) so an exposed proxy is never tokenless;
/// disabling it does **not** move an already-bound non-loopback address back to
/// loopback (the user changes the address explicitly afterwards). Rebuilds tray.
#[tauri::command]
pub fn set_expose_to_network(app: AppHandle, enabled: bool) -> Result<StateView, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state.set_expose_to_network(enabled);
    let _ = crate::tray::apply_menu(&app);
    Ok(state_view(&state))
}

/// Replaces the gateway token with a freshly generated one (so a leaked token
/// can be rotated). Returns the updated state including the new token.
#[tauri::command]
pub fn regenerate_proxy_token(app: AppHandle) -> Result<StateView, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state.regenerate_proxy_token();
    Ok(state_view(&state))
}

#[tauri::command]
pub fn set_api_key(state: State<'_, Arc<AppState>>, key: String) {
    state.set_api_key(key);
}

/// Clears the in-memory API key (the settings window's "forget" action).
#[tauri::command]
pub fn forget_api_key(state: State<'_, Arc<AppState>>) {
    state.set_api_key("");
}

/// Fetches the model list from the configured endpoint's `/models`, stores it,
/// and rebuilds the tray menu. Returns the fetched models for the UI.
#[tauri::command]
pub async fn refresh_models(app: AppHandle) -> Result<Vec<ModelInfo>, String> {
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
    /// All agents the launcher knows about.
    pub const ALL: &'static [Agent] = &[Agent::Copilot, Agent::Codex];

    /// Parses the agent id sent from the UI / tray (e.g. "copilot", "codex").
    pub fn from_id(id: &str) -> Option<Agent> {
        Agent::ALL.iter().copied().find(|a| a.id() == id)
    }

    /// Stable id used by the UI / tray and in `run::<id>` menu ids.
    pub fn id(self) -> &'static str {
        match self {
            Agent::Copilot => "copilot",
            Agent::Codex => "codex",
        }
    }

    /// Human-friendly name shown on buttons / menu entries.
    pub fn label(self) -> &'static str {
        match self {
            Agent::Copilot => "Copilot",
            Agent::Codex => "Codex",
        }
    }

    /// The OpenAI-compatible API this agent talks. The agent can only be
    /// launched when the configured upstream serves this API.
    pub fn api(self) -> &'static str {
        match self {
            Agent::Copilot => "chat",
            Agent::Codex => "responses",
        }
    }
}

/// Agent descriptor sent to the UI, with availability resolved against the
/// upstream APIs declared in the configuration.
#[derive(Serialize)]
pub struct AgentInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub api: &'static str,
    /// True when the configured upstream serves this agent's API.
    pub enabled: bool,
}

#[tauri::command]
pub fn list_agents(state: State<'_, Arc<AppState>>) -> Vec<AgentInfo> {
    Agent::ALL
        .iter()
        .map(|&agent| AgentInfo {
            id: agent.id(),
            label: agent.label(),
            api: agent.api(),
            enabled: agent_supported(&state, agent),
        })
        .collect()
}

/// Whether the active endpoint serves the API this agent needs. Only one API is
/// active at a time (derived from the endpoint URL's suffix).
pub(crate) fn agent_supported(state: &AppState, agent: Agent) -> bool {
    state
        .active_api()
        .map(|a| a.as_str() == agent.api())
        .unwrap_or(false)
}

#[tauri::command]
pub fn run_agent(state: State<'_, Arc<AppState>>, agent: String) -> Result<(), String> {
    let kind = Agent::from_id(&agent).ok_or_else(|| format!("unknown agent: {agent}"))?;
    if !agent_supported(&state, kind) {
        let active = state
            .active_api()
            .map(|a| a.as_str().to_string())
            .unwrap_or_else(|| "none (endpoint not configured)".to_string());
        return Err(format!(
            "{} needs a \"{}\" endpoint, but the active endpoint is: {active}",
            kind.label(),
            kind.api(),
        ));
    }
    launch_agent(&state, kind)
}

/// Opens a new terminal with the proxy environment set and starts the selected
/// agent pointed at the proxy. Shared by the tray menu and the settings window.
pub fn launch_agent(state: &AppState, kind: Agent) -> Result<(), String> {
    let base_url = local_base_url(&state.listen_addr());
    spawn_agent(kind, &base_url).map_err(|e| e.to_string())
}

/// The base URL a *local* agent should use to reach the proxy. When the proxy
/// binds beyond loopback (or to a wildcard like `0.0.0.0`, which is not a
/// connectable destination), the local agent still connects via `127.0.0.1` —
/// that keeps it a loopback peer, so it never needs the gateway token.
pub(crate) fn local_base_url(listen_addr: &str) -> String {
    let addr = listen_addr.trim();
    if proxy_core::is_loopback_listen_addr(addr) {
        return format!("http://{addr}");
    }
    let port = listen_port(addr).unwrap_or("8080");
    format!("http://127.0.0.1:{port}")
}

/// Extracts the port from a `host:port` listen address (handling bracketed
/// IPv6 literals like `[::]:8080`).
fn listen_port(addr: &str) -> Option<&str> {
    let addr = addr.trim();
    if let Some(after_bracket) = addr.strip_prefix('[') {
        after_bracket
            .split_once(']')
            .and_then(|(_, rest)| rest.strip_prefix(':'))
    } else {
        addr.rsplit_once(':').map(|(_, port)| port)
    }
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
                .args(["-NoExit", "-Command", &codex_command(base_url)?])
                .env(CODEX_KEY_ENV, "proxy-managed");
        }
    }

    command.spawn()?;
    Ok(())
}

/// Builds the `codex` invocation that points an ephemeral provider at the proxy.
/// `base_url` is wrapped in a PowerShell single-quoted string so that shell
/// metacharacters (`;`, `&`, `|`, `$`, backtick) inside it are treated as
/// literal text rather than executed — defence-in-depth on top of the strict
/// `validate_listen_addr` host whitelist. A single quote in `base_url` would
/// break out of that quoting, so it is rejected outright (it can never be a
/// valid URL anyway).
#[cfg(target_os = "windows")]
fn codex_command(base_url: &str) -> std::io::Result<String> {
    if base_url.contains('\'') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "proxy base URL must not contain a single quote",
        ));
    }
    Ok(format!(
        "codex \
         -c model_provider=proxy \
         -c model_providers.proxy.name=copilot-proxy \
         -c 'model_providers.proxy.base_url={base_url}' \
         -c model_providers.proxy.wire_api=responses \
         -c model_providers.proxy.env_key={CODEX_KEY_ENV} \
         -c model={PROXY_MODEL_LABEL}"
    ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use proxy_core::RuntimeConfig;

    fn state_with_endpoint(url: &str) -> AppState {
        AppState::new(RuntimeConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            endpoint_url: url.to_string(),
            default_model: None,
            ..RuntimeConfig::default()
        })
    }

    #[test]
    fn responses_endpoint_enables_only_codex() {
        let s = state_with_endpoint("https://e.example/v1/responses");
        assert!(!agent_supported(&s, Agent::Copilot));
        assert!(agent_supported(&s, Agent::Codex));
    }

    #[test]
    fn chat_endpoint_enables_only_copilot() {
        let s = state_with_endpoint("https://e.example/v1/chat/completions");
        assert!(agent_supported(&s, Agent::Copilot));
        assert!(!agent_supported(&s, Agent::Codex));
    }

    #[test]
    fn unconfigured_endpoint_enables_no_agent() {
        let s = state_with_endpoint("");
        assert!(!agent_supported(&s, Agent::Copilot));
        assert!(!agent_supported(&s, Agent::Codex));
    }

    #[test]
    fn endpoint_validation_rejects_v1_only() {
        // The /v1-only URL is the exact mistake the new model guards against.
        assert!(proxy_core::validate_endpoint_url("https://e.example/v1").is_err());
        assert!(proxy_core::validate_endpoint_url("https://e.example/v1/responses").is_ok());
        assert!(proxy_core::validate_listen_addr("127.0.0.1:8080").is_ok());
        assert!(proxy_core::validate_listen_addr("nope").is_err());
    }

    #[test]
    fn listen_addr_changed_compares_trimmed() {
        assert!(!listen_addr_changed("127.0.0.1:8080", "127.0.0.1:8080"));
        assert!(!listen_addr_changed("127.0.0.1:8080", "  127.0.0.1:8080  "));
        assert!(listen_addr_changed("127.0.0.1:8080", "127.0.0.1:9090"));
    }

    #[test]
    fn local_base_url_keeps_loopback_and_rewrites_wildcard() {
        // Loopback addresses are used verbatim.
        assert_eq!(local_base_url("127.0.0.1:8080"), "http://127.0.0.1:8080");
        assert_eq!(local_base_url("localhost:9000"), "http://localhost:9000");
        // Wildcard / non-loopback binds are reached locally via 127.0.0.1.
        assert_eq!(local_base_url("0.0.0.0:8080"), "http://127.0.0.1:8080");
        assert_eq!(local_base_url("192.168.1.10:1234"), "http://127.0.0.1:1234");
        assert_eq!(local_base_url("[::]:7000"), "http://127.0.0.1:7000");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn codex_command_single_quotes_base_url() {
        // The base URL must be wrapped in single quotes so shell metacharacters
        // inside it cannot execute.
        let cmd = codex_command("http://127.0.0.1:8080").unwrap();
        assert!(cmd.contains("-c 'model_providers.proxy.base_url=http://127.0.0.1:8080'"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn codex_command_rejects_single_quote() {
        // A quote would break out of the quoting — reject it outright.
        assert!(codex_command("http://127.0.0.1:8080'whoami").is_err());
    }
}
