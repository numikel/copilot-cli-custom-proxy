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
default_model = "model-a"
models = ["model-a", "model-b", "model-c"]
```

`config.toml` is in `.gitignore` (it holds your private endpoint / model names).
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
- open **"Ustaw klucz API…"** (Set API key) to paste your key,
- choose "Zakończ" (Quit) to exit.

## Configuring GitHub Copilot CLI

Point Copilot at the local proxy as its endpoint (the `openai` type is the default):

```powershell
set COPILOT_PROVIDER_BASE_URL=http://127.0.0.1:8080
set COPILOT_MODEL=placeholder   # replaced by the proxy anyway
copilot
```

`COPILOT_PROVIDER_API_KEY` is not needed — the proxy injects the key from memory.
Use `http://127.0.0.1:8080` without `/v1`: Copilot appends `/chat/completions`,
and the proxy forwards that path to `corporate_base_url`.

## Tests and demo (run on any platform)

The core is GUI-independent:

```bash
cargo test -p proxy-core                 # tests: model swap, auth, missing key (502), streaming
cargo run -p proxy-core --example demo   # end-to-end demo against a stub endpoint
```

## Notes

- **OpenAI-compatible** endpoints (Chat Completions API) are supported.
- All models share a single `corporate_base_url`; the proxy only changes the `model` field.
- The API key lives in memory only — re-enter it after restarting the app.
