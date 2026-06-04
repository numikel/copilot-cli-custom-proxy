# CLAUDE.md — copilot-cli-custom-proxy

Local OpenAI-compatible proxy (Windows tray app) that live-swaps the LLM model
for CLI agents (Copilot, Codex) without restarting the terminal.

## Architecture

Cargo workspace, two members:

- **`proxy-core/`** — GUI-independent core (fully testable). Axum reverse proxy
  that rewrites the `model` field and injects the API key.
  - `state.rs` — `AppState` (shared `Arc`): models `Vec<ModelInfo>`, selected
    model, in-memory `SecretString` key, `RequestLog`. Helpers: `models()`,
    `model_ids()`, `chat_model_ids()`, `set_models()`.
  - `models.rs` — `ModelInfo { id, chat, kind }`, `ModelKind`, and
    `classify_model(id)` (pure, id-heuristic, unit-tested).
  - `proxy.rs` — Axum router, request/response rewriting, `fetch_models()`
    (returns `Vec<ModelInfo>`).
  - `config.rs` — `Config` from `config.toml` (`upstream_apis` gates agents).
  - `ui_state.rs` — `ui_state.json` (non-secret prefs) next to `config.toml`;
    `AppState` keeps a per-endpoint tray-visible model set (`visible_model_ids()`,
    `set_visible_models()`; `None` = all chat models). `ui_state.json` is
    gitignored.
- **`src-tauri/`** — Tauri v2 app.
  - `tray.rs` — native tray menu (status line, **"Models" submenu** of the
    endpoint's visible chat models, Refresh, Run <agent> for supported agents,
    Settings, Quit) + two-state icon (`update_tray_icon`, PNGs `include_bytes!`d).
    Models are in a submenu so first-level items stay reachable with huge catalogs.
  - `commands.rs` — `#[tauri::command]`s: `get_state`, `set_api_key`,
    `forget_api_key`, `set_model`, `run_agent`, `list_agents`, `refresh_models`,
    `set_visible_models`. `Agent` enum + `agent_supported()` gate. `StateView`
    is the JS↔Rust contract.
  - `main.rs` — Builder, background proxy spawn, CloseRequested → hide to tray.
  - `dist/` — settings **webview** (vanilla JS, no bundler; `withGlobalTauri`).
    `index.html` + `styles.css` (ported 1:1 from the design) + `app.js` (state
    machine) + `fonts/` (local IBM Plex woff2).

## Conventions / gotchas

- **Frontend is vanilla JS** — use `window.__TAURI__.core.invoke` and
  `window.__TAURI__.window.getCurrentWindow()`. **No** ES `import` / bundler.
- **CSP is `'self'`** — no CDNs. Fonts are bundled in `dist/fonts/`.
- Window is **frameless** (`decorations:false`); the custom title bar uses
  `data-tauri-drag-region`. Window controls need `core:window:allow-*` perms in
  `capabilities/default.json`.
- Runtime tray-icon swap needs the `image-png` feature on `tauri`.
- `dist/` may be read-blocked by local permission settings; create files fresh
  (delete + Write) rather than editing in place when that happens.
- The API key is **in-memory only** — never persist it.

## Validate

```bash
cargo test -p proxy-core        # unit + integration (classification, swap, auth, streaming)
cargo check --all-targets
cargo clippy --all-targets
cargo tauri dev                 # manual (needs config.toml next to it)
```

Version is shared via `[workspace.package]`; bump it **and** `tauri.conf.json`.
