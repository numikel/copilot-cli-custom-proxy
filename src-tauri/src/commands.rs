use proxy_core::{AppState, ModelInfo, RequestLog};
use serde::Serialize;
use std::sync::{Arc, Mutex};
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
    /// Id of the most recently launched agent whose terminal is still open
    /// (see [`AgentWatch`]), or `None` when no launched terminal is running.
    pub running_agent: Option<String>,
    /// Ready-to-paste PowerShell snippet that launches the agent the active
    /// endpoint can serve (Copilot for chat, Codex for responses), rendered by
    /// the backend so the webview never re-derives the env-var / flag wiring.
    pub manual_command: String,
    /// Manual per-endpoint token-limit override that fills the settings inputs.
    /// `None` means "use the selected model's advertised limit" (which the
    /// webview reads client-side from `models`). The effective value is already
    /// baked into `manual_command`.
    pub token_prompt_override: Option<u32>,
    pub token_output_override: Option<u32>,
}

/// Builds the JS-facing view from current state.
fn state_view(state: &AppState, watch: &AgentWatch) -> StateView {
    let listen_addr = state.listen_addr();
    let active_api = state.active_api().map(|a| a.as_str().to_string());
    // Render the "run manually" snippet for the agent the active endpoint serves,
    // pointed at the address a local client actually reaches, with the selected
    // model's effective token budget baked in.
    let manual_command = manual_command(
        snippet_agent(active_api.as_deref()),
        &local_base_url(&listen_addr),
        state.copilot_token_limits(),
    )
    .unwrap_or_default();
    let (token_prompt_override, token_output_override) = state.token_overrides();
    StateView {
        models: state.models(),
        selected_model: state.selected_model(),
        has_api_key: state.has_api_key(),
        listen_addr,
        endpoint_url: state.endpoint_url(),
        active_api,
        visible_models: state.visible_model_ids(),
        expose_to_network: state.expose_to_network(),
        proxy_token: state.proxy_token(),
        request_log: state.request_log(),
        running_agent: watch.running_id().map(String::from),
        manual_command,
        token_prompt_override,
        token_output_override,
    }
}

#[tauri::command]
pub fn get_state(state: State<'_, Arc<AppState>>, watch: State<'_, AgentWatch>) -> StateView {
    state_view(&state, &watch)
}

/// Sets (or clears) the manual per-endpoint token-limit override the Copilot
/// launch reads into `COPILOT_PROVIDER_MAX_*`. Either value may be `null` to fall
/// back to the selected model's advertised limit. Returns the refreshed state so
/// the webview re-renders the "run manually" snippet with the new budget.
#[tauri::command]
pub fn set_token_limits(
    state: State<'_, Arc<AppState>>,
    watch: State<'_, AgentWatch>,
    prompt: Option<u32>,
    output: Option<u32>,
) -> StateView {
    state.set_token_overrides(prompt, output);
    state_view(&state, &watch)
}

/// One-shot startup warning resolved at boot (e.g. "config.json was corrupt and
/// has been reset"), kept in Tauri-managed state so the settings webview can
/// surface it once on load.
pub struct StartupNotice(pub Option<String>);

#[tauri::command]
pub fn get_startup_warning(notice: State<'_, StartupNotice>) -> Option<String> {
    notice.0.clone()
}

/// Sets the upstream endpoint URL (must end in /chat/completions or /responses)
/// and refreshes the model catalog. To avoid a window where the new URL is
/// paired with a stale/empty catalog (which would forward `"model": ""`), the
/// new endpoint's models are fetched *before* mutating shared state, then the
/// URL and catalog are swapped atomically. On a fetch error the new URL is
/// still committed with an empty catalog (the user picked it deliberately;
/// Refresh recovers once the network is back). Rebuilds the tray.
///
/// The background proxy is **not** restarted here, by design: `proxy_handler`
/// reads `base_url()` from the shared `AppState` per request, so the new
/// endpoint takes effect immediately for every subsequent request. The only
/// window is a single request already in flight (its target URL was built
/// before the swap), which completes against the old upstream — acceptable, and
/// far cheaper than draining in-flight requests on every endpoint change. (A
/// restart *is* needed for `set_listen_addr`, which rebinds the socket.)
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
    Ok(state_view(&state, &app.state::<AgentWatch>()))
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
        return Ok(state_view(&state, &app.state::<AgentWatch>()));
    }

    // Bind the new address before touching the running server. A different
    // address never collides with the old one; a failure here leaves the old
    // server intact (safe rollback) and is reported to the UI. The std socket is
    // handed to `ProxyTask::spawn`, which registers it with the reactor inside
    // the runtime.
    let listener = std::net::TcpListener::bind(&addr)
        .map_err(|e| format!("cannot bind proxy to {addr}: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("cannot configure proxy socket {addr}: {e}"))?;

    state.set_listen_addr(addr);
    let task = app.state::<crate::lifecycle::ProxyTask>();
    task.stop().await;
    task.spawn(listener, state.clone());

    let _ = crate::tray::apply_menu(&app);
    Ok(state_view(&state, &app.state::<AgentWatch>()))
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
    Ok(state_view(&state, &app.state::<AgentWatch>()))
}

/// Replaces the gateway token with a freshly generated one (so a leaked token
/// can be rotated). Returns the updated state including the new token.
#[tauri::command]
pub fn regenerate_proxy_token(app: AppHandle) -> Result<StateView, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state.regenerate_proxy_token();
    Ok(state_view(&state, &app.state::<AgentWatch>()))
}

/// Stores the upstream API key in memory and rebuilds the tray so its icon
/// reflects readiness (active once a key is set and a model is selected).
#[tauri::command]
pub fn set_api_key(app: AppHandle, key: String) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state.set_api_key(key);
    let _ = crate::tray::apply_menu(&app);
}

/// Clears the in-memory API key (the settings window's "forget" action) and
/// rebuilds the tray so its icon drops back to the idle state.
#[tauri::command]
pub fn forget_api_key(app: AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state.set_api_key("");
    let _ = crate::tray::apply_menu(&app);
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

/// Selects the active model (from the settings window) and rebuilds the tray so
/// its checkmark and icon match — the same refresh the tray's own model handler
/// does. The choice is persisted per-endpoint by `set_selected_model`.
#[tauri::command]
pub fn set_model(app: AppHandle, model: String) -> Result<StateView, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if state.set_selected_model(model.clone()) {
        let _ = crate::tray::apply_menu(&app);
        // Return the refreshed state so the webview re-renders the "run manually"
        // snippet, whose token budget tracks the selected model.
        let watch = app.state::<AgentWatch>();
        Ok(state_view(&state, &watch))
    } else {
        Err(format!("unknown model: {model}"))
    }
}

/// The model identifier passed to the launched CLI. Its value is arbitrary —
/// the proxy rewrites the `model` field on every request — so we use a friendly
/// label that makes it obvious traffic flows through this switcher.
/// Placeholder model id handed to the agent CLIs. Arbitrary — the proxy rewrites
/// the `model` field to the actually-selected upstream model. Read by both the
/// Windows launcher and the cross-platform snippet renderer.
const PROXY_MODEL_LABEL: &str = "copilot-proxy-model";

// Environment variables the GitHub Copilot CLI reads its BYOK provider config
// from. Shared by the Windows launcher (`spawn_agent`) and the snippet renderer
// (`manual_command`) so the displayed command can never drift from the launch.
const COPILOT_BASE_URL_ENV: &str = "COPILOT_PROVIDER_BASE_URL";
const COPILOT_MODEL_ENV: &str = "COPILOT_MODEL";
// Token-budget env vars: set so Copilot stops warning that the synthetic model
// id is "not in the built-in catalog" and silently using fallback defaults.
const COPILOT_MAX_PROMPT_ENV: &str = "COPILOT_PROVIDER_MAX_PROMPT_TOKENS";
const COPILOT_MAX_OUTPUT_ENV: &str = "COPILOT_PROVIDER_MAX_OUTPUT_TOKENS";

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

/// Tracks the most recently launched agent terminal so the UI's "live" badge
/// reflects reality (the settings webview polls it via [`StateView`]). The
/// semantics are deliberately terminal-scoped: the launched `Child` is the
/// PowerShell window (started with `-NoExit`), so "running" means "the terminal
/// we opened is still open" — the agent inside may exit earlier.
///
/// The slot owns the spawned [`std::process::Child`], and [`Self::running_id`]
/// answers from the **current process state** (`try_wait`) — every poll
/// re-checks reality instead of relying on a one-shot exit notification that
/// could be missed, so a stale "live" can never outlive the process.
///
/// Single-slot by design: launching again (even the other agent) supersedes the
/// previous entry (the superseded terminal keeps running — it is just no longer
/// tracked). Trade-off: with two terminals open the badge follows only the most
/// recent one — best effort, acceptable for a status pill.
#[derive(Default)]
pub struct AgentWatch(Mutex<Option<RunningAgent>>);

struct RunningAgent {
    agent: Agent,
    child: std::process::Child,
}

impl AgentWatch {
    /// Records a launched terminal, superseding any previous entry.
    pub fn launch(&self, agent: Agent, child: std::process::Child) {
        *self.0.lock().unwrap() = Some(RunningAgent { agent, child });
    }

    /// Id of the launched agent whose terminal is still open, if any. Checks
    /// the real process state and reaps the slot once the terminal has exited
    /// (an unreadable handle counts as exited).
    pub fn running_id(&self) -> Option<&'static str> {
        let mut slot = self.0.lock().unwrap();
        let mut running = slot.take()?;
        if matches!(running.child.try_wait(), Ok(None)) {
            let id = running.agent.id();
            *slot = Some(running);
            Some(id)
        } else {
            tracing::info!(agent = running.agent.id(), "agent terminal exited");
            None
        }
    }
}

#[tauri::command]
pub fn run_agent(app: AppHandle, agent: String) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
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
    launch_agent(&app, kind)
}

/// Opens a new terminal with the proxy environment set and starts the selected
/// agent pointed at the proxy. Shared by the tray menu and the settings window.
/// The spawned terminal is registered in [`AgentWatch`]; the UI's "live"
/// indicator clears itself because every state poll re-checks the process.
pub fn launch_agent(app: &AppHandle, kind: Agent) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>();
    let base_url = local_base_url(&state.listen_addr());
    let limits = state.copilot_token_limits();
    let child = spawn_agent(kind, &base_url, limits).map_err(|e| e.to_string())?;
    tracing::info!(
        agent = kind.id(),
        pid = child.id(),
        "agent terminal launched"
    );
    app.state::<AgentWatch>().launch(kind, child);
    Ok(())
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
fn spawn_agent(
    kind: Agent,
    base_url: &str,
    limits: (Option<u32>, Option<u32>),
) -> std::io::Result<std::process::Child> {
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
                .env(COPILOT_BASE_URL_ENV, base_url)
                .env(COPILOT_MODEL_ENV, PROXY_MODEL_LABEL);
            // Selected model's token budget, when known (silences the catalog
            // warning); left unset so Copilot keeps its defaults otherwise.
            if let Some(p) = limits.0 {
                command.env(COPILOT_MAX_PROMPT_ENV, p.to_string());
            }
            if let Some(o) = limits.1 {
                command.env(COPILOT_MAX_OUTPUT_ENV, o.to_string());
            }
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
                .env(CODEX_KEY_ENV, CODEX_KEY_VALUE);
        }
    }

    command.spawn()
}

/// Builds the `codex` invocation that points an ephemeral provider at the proxy.
/// `base_url` is wrapped in a PowerShell single-quoted string so that shell
/// metacharacters (`;`, `&`, `|`, `$`, backtick) inside it are treated as
/// literal text rather than executed — defence-in-depth on top of the strict
/// `validate_listen_addr` host whitelist. A single quote in `base_url` would
/// break out of that quoting, so it is rejected outright (it can never be a
/// valid URL anyway).
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
const CODEX_KEY_ENV: &str = "CODEX_PROXY_KEY";
/// Dummy value for `CODEX_KEY_ENV` — the proxy injects the real key from memory.
const CODEX_KEY_VALUE: &str = "proxy-managed";

/// Picks the agent whose launch snippet the settings window shows: the one the
/// active endpoint can serve (Codex for a responses endpoint, Copilot for chat),
/// defaulting to Copilot when no endpoint is configured yet.
fn snippet_agent(active_api: Option<&str>) -> Agent {
    match active_api {
        Some(api) if api == Agent::Codex.api() => Agent::Codex,
        _ => Agent::Copilot,
    }
}

/// Renders the exact manual PowerShell snippet a user would run to launch
/// `agent` against the proxy at `base_url`. Pure string building (no process
/// spawning), so it compiles and is unit-tested on every platform — it is the
/// single source of truth the settings webview displays, mirroring what
/// `spawn_agent` executes on Windows. Reuses `codex_command` for the Codex line,
/// so the snippet can never drift from the launched command (and inherits its
/// single-quote rejection).
fn manual_command(
    agent: Agent,
    base_url: &str,
    limits: (Option<u32>, Option<u32>),
) -> std::io::Result<String> {
    Ok(match agent {
        Agent::Copilot => {
            let mut s = format!(
                "$env:{COPILOT_BASE_URL_ENV}=\"{base_url}\"\n\
                 $env:{COPILOT_MODEL_ENV}=\"{PROXY_MODEL_LABEL}\"   # value is arbitrary — the proxy overrides it\n"
            );
            // The selected model's token budget, when known — silences Copilot's
            // "not in the built-in catalog" warning. Omitted when unknown so
            // Copilot keeps its own defaults rather than a wrong guess.
            if let Some(p) = limits.0 {
                s += &format!("$env:{COPILOT_MAX_PROMPT_ENV}=\"{p}\"\n");
            }
            if let Some(o) = limits.1 {
                s += &format!("$env:{COPILOT_MAX_OUTPUT_ENV}=\"{o}\"\n");
            }
            s + "copilot"
        }
        Agent::Codex => format!(
            "$env:{CODEX_KEY_ENV}=\"{CODEX_KEY_VALUE}\"   # dummy — the proxy injects the real key\n{}",
            codex_command(base_url)?
        ),
    })
}

#[cfg(not(target_os = "windows"))]
fn spawn_agent(
    _kind: Agent,
    _base_url: &str,
    _limits: (Option<u32>, Option<u32>),
) -> std::io::Result<std::process::Child> {
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

    /// A quiet long-running process the watch can track (no console window).
    #[cfg(target_os = "windows")]
    fn spawn_sleeper() -> std::process::Child {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("cmd")
            .args(["/c", "ping", "-n", "60", "127.0.0.1"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleeper process")
    }

    #[cfg(target_os = "windows")]
    fn kill(pid: u32) {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output();
    }

    /// Polls `running_id` until the exited process is observed (try_wait may
    /// race the actual process teardown for a moment).
    #[cfg(target_os = "windows")]
    fn wait_until_cleared(watch: &AgentWatch) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while watch.running_id().is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "agent still reported live 10 s after its process exited"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn running_id_clears_once_the_terminal_exits() {
        let watch = AgentWatch::default();
        assert_eq!(watch.running_id(), None);

        let child = spawn_sleeper();
        let pid = child.id();
        watch.launch(Agent::Copilot, child);
        assert_eq!(watch.running_id(), Some("copilot"));

        // Simulates the terminal window being closed: the process dies and the
        // next poll must observe it and clear the badge.
        kill(pid);
        wait_until_cleared(&watch);
        assert_eq!(watch.running_id(), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn running_id_follows_the_latest_launch() {
        let watch = AgentWatch::default();

        let first = spawn_sleeper();
        let first_pid = first.id();
        watch.launch(Agent::Copilot, first);
        assert_eq!(watch.running_id(), Some("copilot"));

        // Single-slot: a newer launch supersedes the previous terminal even
        // though that terminal is still open.
        let second = spawn_sleeper();
        let second_pid = second.id();
        watch.launch(Agent::Codex, second);
        assert_eq!(watch.running_id(), Some("codex"));

        kill(second_pid);
        wait_until_cleared(&watch);

        // The superseded terminal was detached, not killed — clean it up.
        kill(first_pid);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn state_view_reports_running_agent() {
        let s = state_with_endpoint("");
        let watch = AgentWatch::default();
        assert_eq!(state_view(&s, &watch).running_agent, None);

        let child = spawn_sleeper();
        let pid = child.id();
        watch.launch(Agent::Copilot, child);
        assert_eq!(
            state_view(&s, &watch).running_agent.as_deref(),
            Some("copilot")
        );
        kill(pid);
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

    #[test]
    fn codex_command_single_quotes_base_url() {
        // The base URL must be wrapped in single quotes so shell metacharacters
        // inside it cannot execute.
        let cmd = codex_command("http://127.0.0.1:8080").unwrap();
        assert!(cmd.contains("-c 'model_providers.proxy.base_url=http://127.0.0.1:8080'"));
    }

    #[test]
    fn codex_command_rejects_single_quote() {
        // A quote would break out of the quoting — reject it outright.
        assert!(codex_command("http://127.0.0.1:8080'whoami").is_err());
    }

    #[test]
    fn manual_command_copilot_sets_byok_env_then_runs_copilot() {
        let cmd = manual_command(Agent::Copilot, "http://127.0.0.1:8080", (None, None)).unwrap();
        assert!(cmd.starts_with("$env:COPILOT_PROVIDER_BASE_URL=\"http://127.0.0.1:8080\""));
        assert!(cmd.contains("$env:COPILOT_MODEL=\"copilot-proxy-model\""));
        assert!(cmd.trim_end().ends_with("copilot"));
        // The old generic OpenAI snippet (wrong for every agent) must be gone.
        assert!(!cmd.contains("OPENAI_BASE_URL"));
        assert!(!cmd.contains("/v1"));
    }

    #[test]
    fn manual_command_codex_reuses_codex_command_verbatim() {
        let base = "http://127.0.0.1:8080";
        let cmd = manual_command(Agent::Codex, base, (None, None)).unwrap();
        assert!(cmd.starts_with("$env:CODEX_PROXY_KEY=\"proxy-managed\""));
        // Embedding codex_command's output keeps the snippet identical to what
        // spawn_agent actually launches.
        assert!(cmd.contains(&codex_command(base).unwrap()));
    }

    #[test]
    fn manual_command_codex_propagates_single_quote_rejection() {
        assert!(manual_command(Agent::Codex, "http://127.0.0.1:8080'x", (None, None)).is_err());
    }

    #[test]
    fn manual_command_uses_loopback_rewrite_for_wildcard_bind() {
        // The snippet must carry a reachable address, not the wildcard bind.
        let cmd = manual_command(
            Agent::Copilot,
            &local_base_url("0.0.0.0:8080"),
            (None, None),
        )
        .unwrap();
        assert!(cmd.contains("http://127.0.0.1:8080"));
    }

    #[test]
    fn manual_command_copilot_includes_known_token_limits() {
        let cmd = manual_command(
            Agent::Copilot,
            "http://127.0.0.1:8080",
            (Some(120_000), Some(16_000)),
        )
        .unwrap();
        assert!(cmd.contains("$env:COPILOT_PROVIDER_MAX_PROMPT_TOKENS=\"120000\""));
        assert!(cmd.contains("$env:COPILOT_PROVIDER_MAX_OUTPUT_TOKENS=\"16000\""));
        assert!(cmd.trim_end().ends_with("copilot"));
    }

    #[test]
    fn manual_command_copilot_omits_unknown_token_limits() {
        let cmd = manual_command(Agent::Copilot, "http://127.0.0.1:8080", (None, None)).unwrap();
        assert!(!cmd.contains("MAX_PROMPT_TOKENS"));
        assert!(!cmd.contains("MAX_OUTPUT_TOKENS"));
    }

    #[test]
    fn state_view_snippet_follows_active_api() {
        let watch = AgentWatch::default();
        // Chat endpoint → Copilot snippet.
        let chat = state_with_endpoint("https://e.example/v1/chat/completions");
        assert!(state_view(&chat, &watch)
            .manual_command
            .trim_end()
            .ends_with("copilot"));
        // Responses endpoint → Codex snippet.
        let resp = state_with_endpoint("https://e.example/v1/responses");
        assert!(state_view(&resp, &watch)
            .manual_command
            .contains("codex -c model_provider=proxy"));
    }
}
