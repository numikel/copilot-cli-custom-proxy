# Copilot CLI Custom Proxy

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
              corporate_base_url  (OpenAI-compatible endpoint)
                     │  • the response stream is piped straight back
                     ▼
                Copilot CLI
```

- The model list and endpoint address come from `config.toml`.
- **You enter the API key in the settings window** — it is kept only in memory
  (wrapped in `secrecy::SecretString`), never written to disk or to logs.

## Project layout

```
proxy-core/   # core: Axum + Reqwest, model swap, streaming (testable without a GUI)
src-tauri/    # Tauri v2 app: tray, settings window, background server startup
config.example.toml
```

## Configuration

Copy `config.example.toml` to `config.toml` and fill in your own values:

```toml
listen_addr = "127.0.0.1:8080"
corporate_base_url = "https://your-endpoint.example.com/v1"
# default_model and models are optional — see below
```

The **model list is fetched automatically** from `{corporate_base_url}/models`
once you enter your API key (and via the tray's **"Refresh models"**). You can
still pre-seed a static `models` list and a `default_model` in `config.toml` if
you want them to appear before authenticating.

`config.toml` is in `.gitignore` (it holds your private endpoint address).
The app looks for `config.toml` next to the `.exe`, then in the working directory.

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
- pick the active model (applied instantly),
- **"Refresh models"** — re-fetch the model list from the endpoint,
- **"Run Copilot" / "Run Codex"** — open a new terminal with the proxy
  environment already set and start the chosen agent (see the Codex note below),
- open **"Open Settings…"** for the full window (API key, model list, launcher),
- choose **"Quit"** to exit.

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
and the proxy forwards that path to `corporate_base_url`.

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
> (`wire_api = "responses"`); the `chat` wire API was removed. Your
> `corporate_base_url` must therefore expose `/responses`. Chat-only upstreams
> (e.g. a plain Ollama server) won't work with Codex without a Responses→Chat
> translation proxy — that bridge is out of scope for now.

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
- **Keep `listen_addr` on loopback** (`127.0.0.1`). The proxy injects your API
  key into every forwarded request, so binding to a non-loopback address would
  let anything on the network use your key. The app logs a warning if you do.
- Use an `https://` endpoint — a non-HTTPS `corporate_base_url` sends the key
  unencrypted (the app warns about this too).
- The settings window loads only local, static assets under a restrictive CSP.

## Notes

- **OpenAI-compatible** endpoints are supported. Copilot uses Chat Completions
  (`/chat/completions`); Codex uses the Responses API (`/responses`). The proxy
  forwards whichever path the client sends, so the upstream must support it.
- All models share a single `corporate_base_url`; the proxy only changes the `model` field.
- The API key lives in memory only — re-enter it after restarting the app.
