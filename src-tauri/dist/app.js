// Copilot Proxy — settings window logic (vanilla, no bundler).
// Talks to the Rust backend through the global Tauri API (withGlobalTauri:true).
// Everything here reflects REAL proxy state — no simulated traffic.

const invoke = window.__TAURI__.core.invoke;
const appWindow = window.__TAURI__.window.getCurrentWindow();

// ───────────────────────── client state ─────────────────────────
const ui = {
  phase: "no-key", // no-key | loading | ready | error
  errorMsg: "",
  hasApiKey: false,
  models: [], // [{id, chat, kind}]
  selected: "",
  listenAddr: "",
  endpoint: "",
  upstreamApis: [], // ["chat","responses"]
  requestLog: { count: 0, last_model: "", last_path: "", last_target: "", last_status: null },
  agents: [], // [{id,label,api,enabled}]
  running: null, // agent id currently launched from here
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
async function loadState() {
  const s = await invoke("get_state");
  ui.hasApiKey = s.has_api_key;
  ui.models = s.models || [];
  ui.selected = s.selected_model || "";
  ui.listenAddr = s.listen_addr || "";
  ui.endpoint = s.corporate_base_url || "";
  ui.upstreamApis = s.upstream_apis || [];
  ui.requestLog = s.request_log || ui.requestLog;
  ui.visible = new Set(s.visible_models || []);
}

async function loadAgents() {
  ui.agents = await invoke("list_agents");
}

function recomputePhase() {
  if (ui.phase === "loading" || ui.phase === "error") return; // sticky until resolved
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
      <button class="cp-btn cp-btn--ghost cp-btn--sm" id="retry-btn">Retry</button></div>`;
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
          ? `<span class="cp-kindtag cp-kindtag--${esc(m.kind)}">${esc(m.kind)}</span>`
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
function renderAgents() {
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
        <button class="${cls}" data-id="${esc(a.id)}" ${gated ? "disabled" : ""}>
          ${glyph}<span class="cp-agent-name">Run ${esc(a.label)}</span>${right}
        </button>${hint}
      </div>`;
    })
    .join("");
  grid.querySelectorAll(".cp-agentbtn:not([disabled])").forEach((btn) => {
    btn.onclick = () => runAgent(btn.dataset.id);
  });
  renderCommands();
}

function commandText() {
  const listen = ui.listenAddr || "127.0.0.1:8788";
  const supported = ui.agents.filter((a) => a.enabled).map((a) => a.id);
  const first = supported[0] || "copilot";
  const others = supported.slice(1);
  const runLine = others.length ? `${first}        # or: ${others.join(", ")}` : first;
  return `$env:OPENAI_BASE_URL = "http://${listen}/v1"\n$env:OPENAI_API_KEY  = "proxy-managed"\n${runLine}`;
}

function renderCommands() {
  $("cmd-block").textContent = commandText();
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
      const on = ui.upstreamApis.includes(api);
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

// ───────────────────────── full render ─────────────────────────
function render() {
  recomputePhase();
  renderPill();
  renderKey();
  renderModels();
  renderAgents();
  renderStatus();
}

// Lightweight render for polling: never touches the model rows or key input,
// so it won't steal focus or reset the user's filter typing.
function renderLive() {
  renderPill();
  renderStatus();
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

// ───────────────────────── actions ─────────────────────────
async function saveKey() {
  const input = $("api-key-input");
  const key = input.value.trim();
  if (!key) {
    toast("Enter your API key first.", "err");
    return;
  }
  await invoke("set_api_key", { key });
  ui.hasApiKey = true;
  input.value = "";
  await doRefresh(true);
}

async function forgetKey() {
  await invoke("forget_api_key");
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
    await invoke("set_model", { model: id });
    ui.selected = id;
    renderModels();
    renderStatus();
    toast(`Active model: ${shortId(id)}`);
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
  $("theme-toggle").querySelector("use").setAttribute("href", theme === "dark" ? "#i-moon" : "#i-sun");
}

function bind() {
  $("win-min").onclick = () => appWindow.minimize();
  $("win-max").onclick = () => appWindow.toggleMaximize();
  $("win-close").onclick = () => appWindow.close(); // Rust intercepts → hides to tray
  $("theme-toggle").onclick = () =>
    applyTheme(document.documentElement.dataset.theme === "dark" ? "light" : "dark");

  $("save-key-btn").onclick = saveKey;
  $("api-key-input").addEventListener("keydown", (e) => {
    if (e.key === "Enter") saveKey();
  });
  $("forget-key-btn").onclick = forgetKey;
  $("refresh-btn").onclick = () => doRefresh(false);
  $("copy-cmd").onclick = copyCommands;
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
  startPolling();
}

window.addEventListener("DOMContentLoaded", init);
