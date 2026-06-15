// Copilot Proxy — settings window logic (vanilla, no bundler).
// Talks to the Rust backend through the global Tauri API (withGlobalTauri:true).
// Everything here reflects REAL proxy state — no simulated traffic.

const invoke = window.__TAURI__.core.invoke;
const appWindow = window.__TAURI__.window.getCurrentWindow();

// Pure URL/address helpers (API_SUFFIX, detectApi, rewriteSuffix,
// endpointError, listenHost, isLoopbackListenAddr, listenAddrError) live in
// validation.js, loaded before this file — they are also unit-tested with Node.

// ───────────────────────── client state ─────────────────────────
const ui = {
  phase: "no-endpoint", // no-endpoint | no-key | loading | ready | error
  errorMsg: "",
  hasApiKey: false,
  models: [], // [{id, chat, kind, max_prompt_tokens, max_output_tokens}]
  selected: "",
  listenAddr: "",
  exposeToNetwork: false, // allow binding beyond loopback (gated by a token)
  proxyToken: null, // gateway token shown when exposure is on
  endpoint: "", // full upstream URL, including the API suffix
  activeApi: null, // "chat" | "responses" | null (derived from the endpoint URL)
  requestLog: { count: 0, last_model: "", last_path: "", last_target: "", last_status: null },
  agents: [], // [{id,label,api,enabled}]
  running: null, // agent id currently launched from here
  manualCommand: "", // backend-rendered "run manually" snippet for the active agent
  tokenPromptOverride: null, // manual Copilot max-prompt-tokens override (null = auto)
  tokenOutputOverride: null, // manual Copilot max-output-tokens override (null = auto)
  filter: "",
  hideNonChat: true,
  visible: new Set(), // model ids shown in the tray "Models" submenu
};

// Index (within the current filtered display list) of the last visibility
// checkbox toggled — anchor for shift-click range selection.
let lastVisIdx = null;

// ───────────────────────── helpers ─────────────────────────
const $ = (id) => document.getElementById(id);
const esc = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const icon = (id, size = 14) => `<svg width="${size}" height="${size}"><use href="#${id}"/></svg>`;
const shortId = (id) => (id.includes("/") ? id.split("/").pop() : id);

// ───────────────────────── toasts ─────────────────────────
function toast(message, kind = "ok") {
  const host = $("toasts");
  const el = document.createElement("div");
  el.className = `cp-toast cp-toast--${kind}`;
  el.innerHTML = `${icon(kind === "err" ? "i-warn" : "i-check", 15)}<span>${esc(message)}</span>`;
  host.appendChild(el);
  while (host.children.length > 3) host.removeChild(host.firstChild);
  setTimeout(() => {
    el.style.transition = "opacity .2s, transform .2s";
    el.style.opacity = "0";
    el.style.transform = "translateY(6px)";
    setTimeout(() => el.remove(), 220);
  }, 2500);
}

// ───────────────────────── data fetch ─────────────────────────
// Maps a StateView from the backend onto the client state.
function adoptState(s) {
  ui.hasApiKey = s.has_api_key;
  ui.models = s.models || [];
  ui.selected = s.selected_model || "";
  ui.listenAddr = s.listen_addr || "";
  ui.exposeToNetwork = !!s.expose_to_network;
  ui.proxyToken = s.proxy_token || null;
  ui.endpoint = s.endpoint_url || "";
  ui.activeApi = s.active_api || null;
  ui.requestLog = s.request_log || ui.requestLog;
  ui.visible = new Set(s.visible_models || []);
  // Backend-tracked launched-agent terminal — the poll makes this the source of
  // truth for the "live" badge (covers launches from the tray too).
  ui.running = s.running_agent || null;
  // Backend renders the exact "run manually" command for the active agent, so
  // the webview never re-derives the env-var / flag wiring; a successful
  // endpoint/listen change re-renders it via render().
  ui.manualCommand = s.manual_command || "";
  // Manual per-endpoint token-limit override (null = use the model's advertised
  // limit, which the inputs show as a placeholder read from `models`).
  ui.tokenPromptOverride = s.token_prompt_override ?? null;
  ui.tokenOutputOverride = s.token_output_override ?? null;
}

async function loadState() {
  adoptState(await invoke("get_state"));
}

async function loadAgents() {
  ui.agents = await invoke("list_agents");
}

function recomputePhase() {
  if (ui.phase === "loading" || ui.phase === "error") return; // sticky until resolved
  if (!ui.activeApi) {
    ui.phase = "no-endpoint";
    return;
  }
  ui.phase = !ui.hasApiKey ? "no-key" : ui.models.length ? "ready" : "no-key";
}

// ───────────────────────── render: header pill ─────────────────────────
function renderPill() {
  const pill = $("status-pill");
  const dot = pill.querySelector(".cp-pill-dot");
  const text = $("status-pill-text");
  pill.className = "cp-pill";
  dot.className = "cp-pill-dot";
  let variant = "ok",
    label = "ready",
    pulse = false;
  switch (ui.phase) {
    case "no-endpoint":
      variant = "warn";
      label = "set endpoint";
      break;
    case "no-key":
      variant = "warn";
      label = "set API key";
      break;
    case "loading":
      variant = "warn";
      label = "fetching models…";
      pulse = true;
      break;
    case "error":
      variant = "err";
      label = ui.errorMsg || "endpoint error";
      break;
    default:
      variant = "ok";
      label = ui.running ? "live" : "ready";
      pulse = !!ui.running;
  }
  pill.classList.add(`cp-pill--${variant}`);
  if (pulse) dot.classList.add("is-pulse");
  text.textContent = label;
  $("listen-addr").textContent = ui.listenAddr || "—";
}

// ───────────────────────── render: model list ─────────────────────────
// The display filter (search + hide-non-chat) — distinct from `ui.visible`,
// which is the tray-submenu curation.
function filteredModels() {
  const f = ui.filter.trim().toLowerCase();
  return ui.models.filter((m) => {
    if (ui.hideNonChat && !m.chat) return false;
    if (f && !m.id.toLowerCase().includes(f)) return false;
    return true;
  });
}

function chatModelIds() {
  return ui.models.filter((m) => m.chat).map((m) => m.id);
}

function renderVisCount() {
  $("vis-count").textContent = `${ui.visible.size} in tray`;
}

function renderModels() {
  const rows = $("model-rows");
  const list = filteredModels();
  $("model-meta").textContent = `${list.length}/${ui.models.length}`;
  renderVisCount();

  if (ui.phase === "loading") {
    rows.innerHTML =
      `<div class="cp-skel-wrap">` +
      Array.from({ length: 5 })
        .map(() => `<div class="cp-skel"><div class="cp-skel-bar"></div></div>`)
        .join("") +
      `</div><div class="cp-list-status">fetching from upstream…</div>`;
    return;
  }
  if (ui.phase === "error") {
    rows.innerHTML = `<div class="cp-empty cp-empty--err">${icon("i-warn", 26)}
      <div class="cp-empty-title">Couldn't load models</div>
      <div class="cp-empty-sub">${esc(ui.errorMsg || "The upstream returned an error.")}</div>
      <button type="button" class="cp-btn cp-btn--ghost cp-btn--sm" id="retry-btn">Retry</button></div>`;
    $("retry-btn").onclick = doRefresh;
    return;
  }
  if (!ui.models.length) {
    rows.innerHTML = `<div class="cp-empty">${icon("i-search", 22)}
      <div class="cp-empty-title">No models yet</div>
      <div class="cp-empty-sub">Set your API key, then refresh to load the catalog.</div></div>`;
    return;
  }
  if (!list.length) {
    rows.innerHTML = `<div class="cp-empty">${icon("i-search", 22)}
      <div class="cp-empty-title">No matches</div>
      <div class="cp-empty-sub">No models match the current filter.</div></div>`;
    return;
  }

  rows.innerHTML = list
    .map((m, i) => {
      const active = m.id === ui.selected;
      const tag =
        !m.chat && m.kind
          ? (() => {
              if (!MODEL_KINDS.includes(m.kind)) {
                console.warn(`[copilot-proxy] Unknown model kind "${m.kind}" — update MODEL_KINDS in validation.js`);
              }
              return `<span class="${kindTagClass(m.kind)}">${esc(m.kind)}</span>`;
            })()
          : "";
      // Only chat models can appear in the tray, so only they get a checkbox.
      const visBox = m.chat
        ? `<span class="cp-vis-box${ui.visible.has(m.id) ? " is-on" : ""}" data-idx="${i}" data-id="${esc(m.id)}" title="Show in tray (shift-click for a range)">${icon("i-check", 11)}</span>`
        : "";
      return `<div class="cp-modelrow${active ? " is-active" : ""}" data-id="${esc(m.id)}">
        <span class="cp-model-check">${active ? icon("i-check", 13) : ""}</span>
        <span class="cp-model-id">${esc(m.id)}</span>${tag}${visBox}
      </div>`;
    })
    .join("");

  rows.querySelectorAll(".cp-modelrow").forEach((row) => {
    row.onclick = () => pickModel(row.dataset.id);
  });
  rows.querySelectorAll(".cp-vis-box").forEach((box) => {
    box.onclick = (e) => {
      e.stopPropagation(); // don't activate the model
      toggleVisible(box.dataset.id, Number(box.dataset.idx), e.shiftKey);
    };
  });
}

// ───────────────────────── render: agents + commands ─────────────────────────
// The running id the agent grid last drew. The 1.5 s poll re-renders the grid
// only when this goes stale (launch/exit transition) — rebuilding the buttons
// every tick would steal focus and reset hover/active states for no reason.
let renderedRunning = null;

function renderAgents() {
  renderedRunning = ui.running;
  const grid = $("agent-grid");
  $("agent-meta").textContent = `${ui.agents.length} CLI${ui.agents.length === 1 ? "" : "s"}`;
  grid.innerHTML = ui.agents
    .map((a) => {
      const gated = !a.enabled;
      const running = ui.running === a.id;
      let cls = "cp-agentbtn";
      if (gated) cls += " is-gated";
      if (running) cls += " is-running";
      const right = running
        ? `<span class="cp-agent-live"><span class="cp-livedot"></span>live</span>`
        : "";
      const glyph = gated ? icon("i-ban", 14) : icon("i-play", 14);
      const hint = gated
        ? `<div class="cp-gatehint">Needs a <code>${esc(a.api)}</code> endpoint</div>`
        : "";
      return `<div class="cp-agent-wrap">
        <button type="button" class="${cls}" data-id="${esc(a.id)}" ${gated ? "disabled" : ""}>
          ${glyph}<span class="cp-agent-name">Run ${esc(a.label)}</span>${right}
        </button>${hint}
      </div>`;
    })
    .join("");
  grid.querySelectorAll(".cp-agentbtn:not([disabled])").forEach((btn) => {
    btn.onclick = () => runAgent(btn.dataset.id);
  });
  renderCommands();
  renderTokenLimits();
}

function commandText() {
  // The backend renders the exact snippet (correct per active agent, with the
  // reachable base URL — and token limits — baked in); the webview only shows it.
  return ui.manualCommand || "—";
}

function renderCommands() {
  $("cmd-block").textContent = commandText();
}

// Copilot token-budget inputs: shown only for a chat (Copilot) endpoint, since
// COPILOT_PROVIDER_MAX_* don't apply to Codex. Each field holds the manual
// override; its placeholder shows the value auto-detected from the selected
// model (or "auto" when the upstream didn't advertise one).
function renderTokenLimits() {
  const shown = ui.activeApi === "chat";
  $("tok-limits").hidden = !shown;
  $("tok-foot").hidden = !shown;
  if (!shown) return;
  const model = ui.models.find((m) => m.id === ui.selected);
  const fill = (el, override, auto) => {
    if (document.activeElement === el) return; // don't fight the user's typing
    el.value = override != null ? String(override) : "";
    el.placeholder = auto ? String(auto) : "auto";
  };
  fill($("tok-prompt"), ui.tokenPromptOverride, model && model.max_prompt_tokens);
  fill($("tok-output"), ui.tokenOutputOverride, model && model.max_output_tokens);
}

// ───────────────────────── render: status grid ─────────────────────────
function renderStatus() {
  $("status-meta").textContent = ui.running ? "live" : "idle";
  $("status-meta").classList.toggle("cp-sec-meta--live", !!ui.running);

  $("kv-key").innerHTML = ui.hasApiKey
    ? `<span class="cp-badge cp-badge--ok">set</span>`
    : `<span class="cp-badge cp-badge--muted">not set</span>`;
  $("kv-endpoint").textContent = ui.endpoint || "—";
  $("kv-listening").textContent = ui.listenAddr || "—";

  $("kv-apis").innerHTML = ["chat", "responses"]
    .map((api) => {
      const on = ui.activeApi === api;
      return `<span class="cp-apichip ${on ? "is-on" : "is-off"}">${api}${on ? "" : " ✕"}</span>`;
    })
    .join("");

  const log = ui.requestLog;
  $("kv-forwarded").textContent = log.count || 0;

  const last = $("kv-last");
  if (!log.count) {
    last.innerHTML = `<span class="cp-muted">— no requests yet</span>`;
  } else {
    const code = log.last_status || 0;
    const codeCls = code >= 400 ? "err" : "ok";
    last.innerHTML =
      `<span class="cp-last-model" title="${esc(log.last_model)}">${esc(shortId(log.last_model || "—"))}</span>` +
      icon("i-arrow", 13) +
      `<span class="cp-last-ep">edge-proxy</span>` +
      (code ? `<span class="cp-statuscode ${codeCls}">${code}</span>` : "");
  }
}

// ───────────────────────── render: API key section ─────────────────────────
function renderKey() {
  const input = $("api-key-input");
  $("forget-key-btn").hidden = !ui.hasApiKey;
  if (ui.hasApiKey && document.activeElement !== input) {
    input.value = "";
    input.placeholder = "•••••••••••••••• — saved";
  } else if (!ui.hasApiKey) {
    input.placeholder = "paste your API key…";
  }
}

// ───────────────────────── render: endpoint section ─────────────────────────
function renderEndpointSwitch(api) {
  $("api-chat").classList.toggle("is-on", api === "chat");
  $("api-responses").classList.toggle("is-on", api === "responses");
}

function renderEndpoint() {
  const epInput = $("endpoint-input");
  // Don't clobber what the user is typing.
  if (document.activeElement !== epInput) epInput.value = ui.endpoint || "";
  const detected = detectApi(epInput.value) || ui.activeApi;
  renderEndpointSwitch(detected);
  $("endpoint-meta").textContent = ui.activeApi || (ui.endpoint ? "invalid URL" : "not set");

  const listenInput = $("listen-input");
  if (document.activeElement !== listenInput) listenInput.value = ui.listenAddr || "";
  renderExpose();
}

// Reflects the network-exposure opt-in and its gateway token.
function renderExpose() {
  $("expose-toggle").classList.toggle("is-on", ui.exposeToNetwork);
  const row = $("token-row");
  row.hidden = !ui.exposeToNetwork;
  if (ui.exposeToNetwork) {
    const input = $("token-input");
    if (document.activeElement !== input) input.value = ui.proxyToken || "";
  }
}

// ───────────────────────── full render ─────────────────────────
function render() {
  recomputePhase();
  renderPill();
  renderEndpoint();
  renderKey();
  renderModels();
  renderAgents();
  renderStatus();
}

// Lightweight render for polling: never touches the model rows or key input,
// so it won't steal focus or reset the user's filter typing. The agent grid
// re-renders only when the backend-tracked terminal changed — its "live" badge
// lives there, and without this the badge would outlive the closed terminal.
function renderLive() {
  renderPill();
  renderStatus();
  if (renderedRunning !== ui.running) renderAgents();
  $("model-meta").textContent = `${filteredModels().length}/${ui.models.length}`;
}

// ───────────────────────── tray visibility curation ─────────────────────────
function persistVisible() {
  renderVisCount();
  invoke("set_visible_models", { models: Array.from(ui.visible) }).catch((e) =>
    toast(`Couldn't save tray selection: ${e}`, "err")
  );
}

function toggleVisible(id, idx, shift) {
  const target = !ui.visible.has(id); // the state we're switching the clicked box to
  if (shift && lastVisIdx !== null) {
    const list = filteredModels();
    const lo = Math.min(lastVisIdx, idx);
    const hi = Math.max(lastVisIdx, idx);
    for (let i = lo; i <= hi; i++) {
      const m = list[i];
      if (m && m.chat) {
        if (target) ui.visible.add(m.id);
        else ui.visible.delete(m.id);
      }
    }
  } else if (target) {
    ui.visible.add(id);
  } else {
    ui.visible.delete(id);
  }
  lastVisIdx = idx;
  persistVisible();
  renderModels();
}

function selectAllVisible() {
  ui.visible = new Set(chatModelIds());
  lastVisIdx = null;
  persistVisible();
  renderModels();
}

function selectNoneVisible() {
  ui.visible = new Set();
  lastVisIdx = null;
  persistVisible();
  renderModels();
}

// ───────────────────────── actions: endpoint ─────────────────────────
// In-flight guard: applyEndpoint has three triggers (Save button, Enter in the
// input, the API-mode chips) and each call refetches models + adopts state, so
// two overlapping calls could interleave their adoptState in either order.
// Disabling the buttons is feedback; the early return is the actual guard
// (Enter doesn't respect `disabled`).
let endpointBusy = false;

function setEndpointBusy(on) {
  endpointBusy = on;
  ["save-endpoint-btn", "api-chat", "api-responses"].forEach((id) => {
    $(id).disabled = on;
  });
}

async function applyEndpoint(url) {
  if (endpointBusy) return;
  const err = endpointError(url);
  if (err) {
    toast(err, "err");
    return;
  }
  setEndpointBusy(true);
  try {
    // Backend validates, persists, clears the old catalog, and — if a key is
    // set — refetches models, returning the resulting StateView.
    const s = await invoke("set_endpoint", { url: url.trim() });
    adoptState(s);
    // The user just applied a working endpoint — un-stick a stale error phase
    // (recomputePhase keeps "error" until explicitly cleared) so the UI stops
    // showing an outdated failure. Done only on this success path, never in
    // adoptState: the 1.5 s poll also calls adoptState and would mask fresh
    // refresh errors.
    ui.phase = "ready";
    ui.errorMsg = "";
    // Agent gating depends on the active API, which just changed — reload it so
    // the right CLI (Copilot for chat / Codex for responses) enables.
    await loadAgents().catch(() => {});
    render();
    toast(`Endpoint set — ${ui.activeApi || "?"} API.`);
  } catch (e) {
    toast(String(e), "err");
  } finally {
    setEndpointBusy(false);
  }
}

async function applyListen(addr) {
  const a = addr.trim();
  const err = listenAddrError(a);
  if (err) {
    toast(err, "err");
    return;
  }
  if (!isLoopbackListenAddr(a) && !ui.exposeToNetwork) {
    toast('Turn on "expose to network" before binding beyond 127.0.0.1.', "err");
    return;
  }
  try {
    const s = await invoke("set_listen_addr", { addr: a });
    adoptState(s);
    render();
    toast(`Listening on ${ui.listenAddr} — proxy restarted.`);
  } catch (e) {
    toast(String(e), "err");
  }
}

// ───────────────────────── actions: network exposure ─────────────────────────
async function toggleExpose() {
  const enable = !ui.exposeToNetwork;
  try {
    const s = await invoke("set_expose_to_network", { enabled: enable });
    adoptState(s);
    render();
    toast(
      enable
        ? "Network exposure on — remote clients need the token below."
        : "Network exposure off."
    );
  } catch (e) {
    toast(String(e), "err");
  }
}

async function regenToken() {
  try {
    const s = await invoke("regenerate_proxy_token");
    adoptState(s);
    render();
    toast("Gateway token regenerated.");
  } catch (e) {
    toast(String(e), "err");
  }
}

async function copyToken() {
  if (!ui.proxyToken) return;
  try {
    await navigator.clipboard.writeText(ui.proxyToken);
    toast("Token copied.");
  } catch {
    toast("Couldn't access the clipboard.", "err");
  }
}

// ───────────────────────── actions ─────────────────────────
async function saveKey() {
  const input = $("api-key-input");
  const key = input.value.trim();
  if (!key) {
    toast("Enter your API key first.", "err");
    return;
  }
  // Only the invoke is guarded: state mutations happen strictly after success
  // (no premature hasApiKey=true), and doRefresh handles its own errors.
  try {
    await invoke("set_api_key", { key });
  } catch (e) {
    toast(String(e), "err");
    return;
  }
  ui.hasApiKey = true;
  input.value = "";
  await doRefresh(true);
}

async function forgetKey() {
  try {
    await invoke("forget_api_key");
  } catch (e) {
    toast(String(e), "err");
    return;
  }
  ui.hasApiKey = false;
  ui.models = [];
  ui.selected = "";
  ui.phase = "no-key";
  render();
  toast("API key forgotten.");
}

async function doRefresh(afterSave = false) {
  if (!ui.hasApiKey) {
    toast("Set your API key first.", "err");
    return;
  }
  ui.phase = "loading";
  ui.errorMsg = "";
  setRefreshSpinning(true);
  render();
  try {
    const models = await invoke("refresh_models"); // Vec<ModelInfo>
    ui.models = models || [];
    ui.phase = "ready";
    if (!ui.models.some((m) => m.id === ui.selected)) {
      const pref =
        ui.models.find((m) => m.id === "gpt-4o") ||
        ui.models.find((m) => m.chat) ||
        ui.models[0];
      if (pref) {
        ui.selected = pref.id;
        await invoke("set_model", { model: pref.id }).catch(() => {});
      }
    }
    // Pull the refreshed visibility set (the backend resolves defaults).
    await loadState().catch(() => {});
    toast(afterSave ? `Loaded ${ui.models.length} models.` : `Models refreshed — ${ui.models.length}.`);
  } catch (e) {
    ui.phase = "error";
    ui.errorMsg = String(e);
    toast(String(e), "err");
  } finally {
    setRefreshSpinning(false);
    render();
  }
}

function setRefreshSpinning(on) {
  const btn = $("refresh-btn");
  btn.disabled = on;
  btn.querySelector("svg").classList.toggle("cp-spin", on);
}

async function pickModel(id) {
  try {
    // set_model returns the refreshed state so the snippet's token budget and
    // the override placeholders track the newly selected model.
    adoptState(await invoke("set_model", { model: id }));
    renderModels();
    renderStatus();
    renderCommands();
    renderTokenLimits();
    toast(`Active model: ${shortId(id)}`);
  } catch (e) {
    toast(String(e), "err");
  }
}

// Saves (or clears) the manual Copilot token-limit override for this endpoint.
async function applyTokenLimits() {
  const pEl = $("tok-prompt");
  const oEl = $("tok-output");
  const err = tokenLimitError(pEl.value) || tokenLimitError(oEl.value);
  if (err) {
    toast(err, "err");
    return;
  }
  const num = (el) => (el.value.trim() === "" ? null : Number(el.value.trim()));
  try {
    adoptState(await invoke("set_token_limits", { prompt: num(pEl), output: num(oEl) }));
    render();
  } catch (e) {
    toast(String(e), "err");
  }
}

async function runAgent(id) {
  if (!ui.selected) {
    toast("Pick a model first.", "err");
    return;
  }
  const agent = ui.agents.find((a) => a.id === id);
  try {
    await invoke("run_agent", { agent: id });
    ui.running = id;
    render();
    toast(`Started ${agent ? agent.label : id} — traffic now routes through the proxy.`);
    startPolling();
  } catch (e) {
    toast(String(e), "err");
  }
}

async function copyCommands() {
  try {
    await navigator.clipboard.writeText(commandText());
    const btn = $("copy-cmd");
    btn.innerHTML = `${icon("i-check", 13)} copied`;
    setTimeout(() => (btn.innerHTML = `${icon("i-copy", 13)} copy`), 1500);
  } catch {
    toast("Couldn't access the clipboard.", "err");
  }
}

// ───────────────────────── live polling ─────────────────────────
let pollTimer = null;
function startPolling() {
  if (pollTimer) return;
  pollTimer = setInterval(async () => {
    if (document.hidden) return; // window hidden in tray — skip
    try {
      await loadState();
      renderLive();
    } catch {
      /* transient — ignore */
    }
  }, 1500);
}
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) loadState().then(renderLive).catch(() => {});
});

// ───────────────────────── wiring ─────────────────────────
function applyTheme(theme) {
  document.documentElement.dataset.theme = theme;
  localStorage.setItem("cp-theme", theme);
  $("theme-toggle").querySelector("use").setAttribute("href", theme === "dark" ? "#i-sun" : "#i-moon");
}

function bind() {
  $("win-min").onclick = () => appWindow.minimize();
  $("win-max").onclick = () => appWindow.toggleMaximize();
  $("win-close").onclick = () => appWindow.close(); // Rust intercepts → hides to tray
  $("theme-toggle").onclick = () =>
    applyTheme(document.documentElement.dataset.theme === "dark" ? "light" : "dark");

  // Endpoint URL + API mode switch.
  $("save-endpoint-btn").onclick = () => applyEndpoint($("endpoint-input").value);
  $("endpoint-input").addEventListener("keydown", (e) => {
    if (e.key === "Enter") applyEndpoint($("endpoint-input").value);
  });
  $("endpoint-input").addEventListener("input", () => {
    // Live-reflect the detected API on the switch while typing.
    renderEndpointSwitch(detectApi($("endpoint-input").value) || ui.activeApi);
  });
  ["api-chat", "api-responses"].forEach((id) => {
    $(id).onclick = () => {
      const suffix = $(id).dataset.suffix;
      const cur = $("endpoint-input").value.trim() || ui.endpoint;
      if (!cur) {
        toast("Enter the endpoint URL first.", "err");
        return;
      }
      const url = rewriteSuffix(cur, suffix);
      $("endpoint-input").value = url;
      applyEndpoint(url);
    };
  });

  // Listen address.
  $("save-listen-btn").onclick = () => applyListen($("listen-input").value);
  $("listen-input").addEventListener("keydown", (e) => {
    if (e.key === "Enter") applyListen($("listen-input").value);
  });
  $("expose-toggle").onclick = toggleExpose;
  $("copy-token-btn").onclick = copyToken;
  $("regen-token-btn").onclick = regenToken;

  $("save-key-btn").onclick = saveKey;
  $("api-key-input").addEventListener("keydown", (e) => {
    if (e.key === "Enter") saveKey();
  });
  $("forget-key-btn").onclick = forgetKey;
  $("refresh-btn").onclick = () => doRefresh(false);
  $("copy-cmd").onclick = copyCommands;
  ["tok-prompt", "tok-output"].forEach((id) => {
    $(id).addEventListener("change", applyTokenLimits);
    $(id).addEventListener("keydown", (e) => {
      if (e.key === "Enter") applyTokenLimits();
    });
  });
  $("vis-all").onclick = selectAllVisible;
  $("vis-none").onclick = selectNoneVisible;

  const filter = $("model-filter");
  filter.addEventListener("input", () => {
    ui.filter = filter.value;
    $("filter-clear").hidden = !filter.value;
    lastVisIdx = null; // filtering changes row indices — reset the range anchor
    renderModels();
  });
  $("filter-clear").onclick = () => {
    filter.value = "";
    ui.filter = "";
    $("filter-clear").hidden = true;
    lastVisIdx = null;
    renderModels();
    filter.focus();
  };
  $("hide-nonchat").onclick = () => {
    ui.hideNonChat = !ui.hideNonChat;
    $("hide-nonchat").classList.toggle("is-on", ui.hideNonChat);
    lastVisIdx = null;
    renderModels();
  };
}

async function init() {
  applyTheme(localStorage.getItem("cp-theme") || "dark");
  bind();
  try {
    await Promise.all([loadState(), loadAgents()]);
  } catch (e) {
    toast(`Failed to read state: ${e}`, "err");
  }
  render();
  // One-shot startup warning from the backend (e.g. config migration notice).
  // Silent catch: command may not be registered yet in dev builds.
  try {
    const w = await invoke("get_startup_warning");
    if (w) toast(w, "err");
  } catch {}
  startPolling();
}

window.addEventListener("DOMContentLoaded", init);
