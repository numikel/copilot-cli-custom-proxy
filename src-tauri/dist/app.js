const invoke = window.__TAURI__.core.invoke;
const $ = (id) => document.getElementById(id);

// Heuristic for non-chat models (embeddings, rerankers, audio, etc.).
const NON_CHAT = /(embed|embedding|rerank|whisper|\bbge\b|bge-|tts|stt|voice|audio|moderation|guard)/i;
const MODEL_LABEL = "copilot-proxy-model";

let allModels = [];
let selected = "";

function toast(msg, isError) {
  const el = $("toast");
  el.textContent = msg;
  el.style.color = isError ? "#f87171" : "#4ade80";
  clearTimeout(toast._t);
  toast._t = setTimeout(() => (el.textContent = ""), 2600);
}

function commandsFor(listenAddr) {
  return (
    '$env:COPILOT_PROVIDER_BASE_URL="http://' + listenAddr + '"\n' +
    '$env:COPILOT_MODEL="' + MODEL_LABEL + '"\n' +
    "copilot"
  );
}

function visibleModels() {
  const q = $("search").value.trim().toLowerCase();
  const hide = $("hidenonchat").checked;
  return allModels.filter((m) => {
    if (hide && NON_CHAT.test(m) && m !== selected) return false;
    if (q && !m.toLowerCase().includes(q)) return false;
    return true;
  });
}

function renderModels() {
  const list = $("modellist");
  const models = visibleModels();
  if (allModels.length === 0) {
    list.innerHTML =
      '<div class="empty">No models yet — set your API key, then refresh.</div>';
    return;
  }
  if (models.length === 0) {
    list.innerHTML = '<div class="empty">No models match the filter.</div>';
    return;
  }
  list.innerHTML = "";
  for (const m of models) {
    const row = document.createElement("div");
    row.className = "model" + (m === selected ? " active" : "");
    row.innerHTML = '<span class="check">✓</span><span class="name"></span>';
    row.querySelector(".name").textContent = m;
    row.addEventListener("click", async () => {
      try {
        await invoke("set_model", { model: m });
        selected = m;
        renderModels();
        toast("Active model: " + m);
      } catch (err) {
        toast(String(err), true);
      }
    });
    list.appendChild(row);
  }
}

async function refresh() {
  const s = await invoke("get_state");

  $("endpoint").textContent = s.corporate_base_url;
  $("listen").textContent = s.listen_addr;
  $("cmds").textContent = commandsFor(s.listen_addr);

  const keyEl = $("keystatus");
  keyEl.textContent = s.has_api_key ? "set" : "not set";
  keyEl.className = "badge " + (s.has_api_key ? "ok" : "warn");

  const ready = s.has_api_key && s.models.length > 0;
  $("proxydot").className = "dot " + (ready ? "on" : "off");
  $("proxytext").textContent = ready
    ? "proxy ready"
    : s.has_api_key
    ? "no models"
    : "set API key";

  // Only re-render the list when the data actually changed (avoids flicker).
  const key = s.models.join("|") + "::" + s.selected_model;
  if (key !== renderModels._key) {
    renderModels._key = key;
    allModels = s.models;
    selected = s.selected_model;
    renderModels();
  }

  const log = s.request_log;
  $("reqcount").textContent = log.count;
  if (log.count > 0) {
    const status = log.last_status === null ? "pending" : log.last_status;
    $("lastreq").textContent =
      log.last_model + " → " + log.last_target + " (" + status + ")";
  }
}

async function fetchModels(announce) {
  try {
    const models = await invoke("refresh_models");
    if (announce) toast("Fetched " + models.length + " models.");
    await refresh();
  } catch (err) {
    if (announce) toast(String(err), true);
  }
}

$("save").addEventListener("click", async () => {
  const key = $("apikey").value;
  await invoke("set_api_key", { key });
  $("apikey").value = "";
  if (key) {
    toast("API key saved — fetching models…");
    await fetchModels(true);
  } else {
    toast("API key cleared.");
    await refresh();
  }
});

$("refresh").addEventListener("click", () => fetchModels(true));
$("search").addEventListener("input", renderModels);
$("hidenonchat").addEventListener("change", renderModels);

$("runcopilot").addEventListener("click", async () => {
  try {
    await invoke("run_copilot");
    toast("Launched Copilot in a new terminal.");
  } catch (err) {
    toast(String(err), true);
  }
});

$("copycmds").addEventListener("click", async () => {
  try {
    await navigator.clipboard.writeText($("cmds").textContent);
    toast("Commands copied to clipboard.");
  } catch (err) {
    toast("Copy failed — select the text manually.", true);
  }
});

refresh();
// Poll so traffic stats stay live; the model list only re-renders on change.
setInterval(refresh, 1500);
