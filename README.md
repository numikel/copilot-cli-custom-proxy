# Copilot CLI Custom Proxy

[![CI](https://github.com/numikel/copilot-cli-custom-proxy/actions/workflows/ci.yml/badge.svg)](https://github.com/numikel/copilot-cli-custom-proxy/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
![Platform: Windows](https://img.shields.io/badge/platform-Windows-0078D6?logo=windows)
![Rust](https://img.shields.io/badge/Rust-2021-orange?logo=rust)
![Tauri v2](https://img.shields.io/badge/Tauri-v2-24C8DB?logo=tauri)

A local HTTP proxy that lives in the system tray (Windows) for **GitHub Copilot
CLI**. It intercepts requests, **swaps the LLM model on the fly**, and forwards
them to a configured OpenAI-compatible endpoint — without restarting your
terminal session.

In BYOK mode, Copilot CLI fixes the model at startup (`COPILOT_MODEL`). This
proxy lets you switch the model from the tray menu while you work.

## How it works

```
Copilot CLI ──▶ http://127.0.0.1:8080  (this proxy)
                     │  • replaces the "model" field with the model picked in the tray
                     │  • injects Authorization: Bearer <key held in memory>
                     │  • forwards the remaining headers (except Host)
                     ▼
              endpoint base  (OpenAI-compatible endpoint)
                     │  • the response stream is piped straight back
                     ▼
                Copilot CLI
```

- **Everything is configured in the settings window** — the endpoint URL and the
  local listen address are set there and persist to `config.json` (next to the
  executable). No `config.toml` editing required.
- **You enter the API key in the settings window** — it is kept only in memory
  (wrapped in `secrecy::SecretString`), never written to disk or to logs.

## Project layout

```
proxy-core/   # core: Axum + Reqwest, model swap, streaming (testable without a GUI)
src-tauri/    # Tauri v2 app: tray, settings window, background server startup
config.example.toml
```

## Configuration

Everything is configured **in the settings window** — there is nothing to edit by
hand. On first run (no config yet) the app starts with defaults and opens the
settings window automatically. Your choices persist to `config.json` next to the
executable (gitignored, as it holds your private endpoint URL).

In the **Endpoint** section you set:

- **Endpoint URL** — the *full* upstream URL, including the API suffix, e.g.
  `https://openrouter.ai/api/v1/chat/completions` or
  `https://openrouter.ai/api/v1/responses`. **Do not stop at `/v1`** — the URL
  suffix is what tells the proxy whether this is a Chat Completions or a Responses
  endpoint.
- **Chat completions ⟷ Responses switch** — one API is active at a time. Flipping
  the switch rewrites the URL suffix; the active API decides which CLI agent you
  can launch (Copilot for chat, Codex for responses).
- **Listen address** — the local `host:port` the proxy binds (the host is
  restricted to a strict character set). Changing it restarts only the background
  proxy task (no app/terminal restart): the new address is bound first, so if the
  port is already in use the error is shown in the window and the running proxy is
  left intact. Re-confirming the same address does nothing.

The **model list is fetched automatically** from `{endpoint base}/models` (the
endpoint URL minus the API suffix) once you enter your API key, and via the
tray's **"Refresh models"**.

### Which agents you can launch

Different CLI agents speak different OpenAI-compatible APIs. The active endpoint
serves exactly one API (derived from its URL suffix), so the app enables only the
matching agent:

| Endpoint suffix | API | Agent |
|-----------------|-----|-------|
| `/chat/completions` | chat | GitHub Copilot CLI |
| `/responses` | responses | Codex CLI |

The other agent is shown disabled (settings window) or hidden (tray), so you
never point a CLI at an endpoint that can't answer it. To use the other agent,
flip the switch (and ensure your upstream serves that API).

### Optional `config.toml` seed

`config.toml` is **no longer required**. If present on first run (and no
`config.json` exists yet), it is migrated into `config.json` once and then
ignored — useful for upgrading an older install or pre-baking a deployment. Copy
`config.example.toml` to `config.toml` for the seed format. The app looks for
config next to the `.exe`, then in the working directory.

## Build and run (Windows)

Requirements: [Rust](https://rustup.rs) and the
[Tauri v2 system prerequisites](https://tauri.app/start/prerequisites/)
(WebView2 ships with Windows 10/11).

```powershell
# development mode (with a console and logs)
cargo install tauri-cli --version "^2.0"
cargo tauri dev

# production build (.exe / installer)
cargo tauri build
```

On launch the app minimizes to the tray. From the tray menu you can:
- pick the active model (applied instantly, and remembered per-endpoint across
  restarts),
- **"Refresh models"** — re-fetch the model list from the endpoint,
- **"Run Copilot" / "Run Codex"** — open a new terminal with the proxy
  environment already set and start the chosen agent (see the Codex note below),
- open **"Open Settings…"** for the full window (API key, model list, launcher),
- choose **"Quit"** to exit.

The tray icon has two states: an accent-filled glyph when the proxy is ready (a
key is set and a model is selected) and a muted outline when it is idle. Models
live in a **"Models ▸" submenu** (not the first level) so "Open Settings…" and
"Quit" stay reachable even with hundreds of models. You choose **which** models
appear in that submenu in the settings window (see below) — the full catalog is
always available there.

## Settings window

The settings window is a small, single-purpose webview (vanilla HTML/CSS/JS — no
bundler, served from `src-tauri/dist/` under a restrictive `'self'` CSP). It is a
**frameless** window (`decorations: false`) with its own title bar, and ships a
**dark/light theme toggle** (defaults to dark; the choice is remembered in
`localStorage`). Fonts (IBM Plex Sans/Mono) are bundled locally, so the UI needs
no network access. It has five sections:

- **Endpoint** — the full upstream URL with a **Chat completions ⟷ Responses**
  switch (one active at a time; the switch rewrites the URL suffix), plus the
  local **listen address**. Both are validated and persisted to `config.json`;
  changing the listen address restarts the proxy task. An **"expose to network"**
  toggle lets you bind beyond loopback on purpose — see
  [Exposing the proxy on your network](#exposing-the-proxy-on-your-network).
- **API key** — paste your key (held in memory only; a **forget** link clears it).
- **Model** — searchable list of the upstream catalog with a **"hide non-chat"**
  toggle. Models are classified in `proxy-core` (chat vs the `embed` / `image` /
  `audio` / `rerank` / `moderation` families) and tagged accordingly; clicking a
  model applies it instantly — and the choice is **remembered per-endpoint** in
  `ui_state.json`, so each upstream restores its own active model after a restart
  (and switching endpoints back and forth). Each chat model has a **"show in tray"
  checkbox** that controls whether it appears in the tray's Models submenu — with
  **all / none** shortcuts and **shift-click** range selection. That tray-visibility
  choice is likewise saved per-endpoint to `ui_state.json` (next to `config.toml`).
- **Start agent** — one button per known agent, gated against the active
  endpoint's API (the incompatible agent is disabled with a tooltip explaining
  which API it needs). A copy-able PowerShell command block is shown too.
- **Status** — live, real values polled from the proxy (~1.5 s): the configured
  endpoint, the active API, the **forwarded** request counter, and the
  **last** request (model → endpoint → status code).

## Configuring GitHub Copilot CLI

The easiest way is the **"Run Copilot"** button (tray or settings window) — it
launches a terminal with the environment already pointed at the proxy.

Alternatively, the settings window has a **"Copy commands"** button (and shows
the commands as selectable text) so you can paste them into your own PowerShell —
handy if `copilot` is not on the PATH of the launched shell:

```powershell
$env:COPILOT_PROVIDER_BASE_URL="http://127.0.0.1:8080"
$env:COPILOT_MODEL="copilot-proxy-model"   # value is arbitrary — the proxy overrides it
copilot
```

`COPILOT_PROVIDER_API_KEY` is not needed — the proxy injects the key from memory.
Use `http://127.0.0.1:8080` without `/v1`: Copilot appends `/chat/completions`,
and the proxy forwards that path to your endpoint base (the endpoint URL minus its
API suffix).

### Configuring Codex CLI

The **"Run Codex"** button (tray or settings window) launches `codex` with an
ephemeral provider pointed at the proxy — no edits to your `~/.codex/config.toml`.
The equivalent manual commands are shown under **"Copy commands"**:

```powershell
$env:CODEX_PROXY_KEY="proxy-managed"   # dummy — the proxy injects the real key
codex -c model_provider=proxy `
  -c model_providers.proxy.base_url="http://127.0.0.1:8080" `
  -c model_providers.proxy.wire_api=responses `
  -c model_providers.proxy.env_key=CODEX_PROXY_KEY `
  -c model=copilot-proxy-model
```

> **Important:** since February 2026 Codex speaks **only the Responses API**
> (`wire_api = "responses"`); the `chat` wire API was removed. Your endpoint must
> therefore be a **`/responses`** URL — set it in the settings window (or flip the
> switch to **Responses**). When the active endpoint is a chat one,
> **"Run Codex" is disabled**, so you never point Codex at an endpoint that can't
> answer it. Chat-only upstreams (e.g. a plain Ollama server) would need a
> Responses→Chat translation proxy, which is out of scope for now.

### Verifying what Copilot really talks to

The proxy logs every forwarded request (model + target URL + status). In dev
mode (`cargo tauri dev`) you see them in the console, e.g.:

```
INFO forwarding request method=POST path=/chat/completions model=model-b target=https://your-endpoint.example.com/v1/chat/completions
INFO upstream responded status=200 OK model=model-b
```

The settings window also shows a live **"Requests forwarded"** counter and the
**last request** (model → endpoint → status), which works in release builds too.

## Tests and demo (run on any platform)

The core is GUI-independent:

```bash
cargo test -p proxy-core                 # tests: model swap, auth, missing key (502), streaming
cargo run -p proxy-core --example demo   # end-to-end demo against a stub endpoint
```

## CI / prebuilt executable

A GitHub Actions workflow (`.github/workflows/ci.yml`) runs on every push:

- **test** (Linux) — runs the core tests and type-checks the Tauri app.
- **build-windows** — builds the release `.exe` and the MSI/NSIS installers, and
  uploads them as workflow **artifacts** (downloadable from the run's summary page).

Push a `v*` tag (e.g. `v0.1.0`) to also attach the binaries to a GitHub Release.

## Security

- The API key is kept in memory only (wrapped in `secrecy::SecretString`):
  never written to disk, never logged, never returned to the UI.
- **The proxy is loopback-only by default** (`127.0.0.1`). It injects your API
  key into every forwarded request, so a non-loopback bind would let anything on
  the network use your key. Binding beyond loopback is therefore an explicit
  opt-in protected by a gateway token — see
  [Exposing the proxy on your network](#exposing-the-proxy-on-your-network).
- Use an `https://` endpoint — a non-HTTPS endpoint URL sends the key
  unencrypted (the app warns about this too).
- Don't embed credentials in the endpoint URL (`https://user:pass@host/…`) — such
  URLs are rejected so a key can't leak into `config.json` or the logs.
- The listen address and endpoint URL are validated both when entered and when
  `config.json` is loaded at startup; an invalid hand-edited value falls back to a
  safe default instead of being trusted.
- The settings window loads only local, static assets under a restrictive CSP.

### Exposing the proxy on your network

By default the proxy binds to `127.0.0.1` and only the local machine can reach
it. To let another device (e.g. a second machine on your LAN) use the proxy:

1. In the settings window, turn on **"expose to network"** under the listen
   address. This generates a **gateway token** and reveals it (with **copy** and
   **regenerate** actions).
2. Set the listen address to a reachable interface — e.g. `0.0.0.0:8080` to bind
   all interfaces. (Without the opt-in, a non-loopback address is rejected, and a
   hand-edited `config.json` with one is reset to loopback on startup.)
3. On the remote client, point it at `http://<this-machine-ip>:8080` and send the
   gateway token in the `Authorization` header: `Authorization: Bearer <token>`.

Loopback clients (including the locally launched Copilot/Codex agents, which
always connect via `127.0.0.1`) never need the token — it gates non-loopback
peers only. The token is a self-generated credential for *this proxy*, separate
from your upstream API key; it is stored in `config.json` so a remote device need
not re-pair after a restart. Regenerate it to revoke access. Even token-gated,
remember the proxy spends your upstream key on behalf of any authorized client —
only expose it on networks you trust.

## Notes

- **OpenAI-compatible** endpoints are supported. Copilot uses Chat Completions
  (`/chat/completions`); Codex uses the Responses API (`/responses`). The proxy
  forwards whichever path the client sends, so the upstream must support it.
- All models share a single endpoint base; the proxy only changes the `model` field.
- The API key lives in memory only — re-enter it after restarting the app.
