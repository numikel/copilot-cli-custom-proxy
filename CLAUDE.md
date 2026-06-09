# CLAUDE.md — copilot-cli-custom-proxy

Local OpenAI-compatible proxy (Windows tray app) that live-swaps the LLM model
for CLI agents (Copilot, Codex) without restarting the terminal.

## Architecture

Cargo workspace, two members:

- **`proxy-core/`** — GUI-independent core (fully testable). Axum reverse proxy
  that rewrites the `model` field and injects the API key.
  - `settings.rs` — `RuntimeConfig { listen_addr, endpoint_url, default_model }`
    persisted to `config.json` (live source of truth). `ApiKind { Chat, Responses }`
    is **derived from the endpoint URL suffix** (`/chat/completions` | `/responses`);
    `base_url()`/`models_url()` strip it. `validate_endpoint_url` (rejects URLs that
    stop at `/v1` **and** URLs with `user:pass@` credentials), `validate_listen_addr`
    (strict host whitelist — RFC 1123 chars or bracketed IPv6 — so the address is
    safe to interpolate into a launched CLI command; port 1–65535).
  - `state.rs` — `AppState` (shared `Arc`): `Mutex<RuntimeConfig>` + `config_path`,
    models `Vec<ModelInfo>`, selected model, in-memory `SecretString` key,
    `RequestLog`. Accessors: `endpoint_url()`, `base_url()`, `models_url()`,
    `active_api()`, `listen_addr()`, `set_endpoint()`, `set_listen_addr()`,
    `swap_endpoint()` (atomic URL + catalog swap, keeps the selection if it
    survives — closes the empty-`model` race), `models()`, `chat_model_ids()`,
    `set_models()`. Persists config on mutation.
  - `models.rs` — `ModelInfo { id, chat, kind }`, `ModelKind`, and
    `classify_model(id)` (pure, id-heuristic, unit-tested).
  - `proxy.rs` — Axum router, request/response rewriting, `fetch_models()`
    (uses `models_url()`) and `fetch_models_from(http, endpoint_url, key)` (probes
    a **candidate** endpoint without mutating state — used by `set_endpoint` so the
    catalog can be swapped atomically); forwards to `base_url() + path` (502 if
    unconfigured).
  - `config.rs` — legacy `Config` from `config.toml`; `into_runtime()` migrates it
    to `RuntimeConfig` (seed only; `config.toml` is optional as of 0.3.0).
  - `ui_state.rs` — `ui_state.json` (non-secret prefs) next to the config;
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
    `set_visible_models`, `set_endpoint`, `set_listen_addr` (both async;
    `set_endpoint` probes + swaps atomically, `set_listen_addr` eagerly binds the
    new address — a bind error surfaces to the UI — and is a no-op when unchanged).
    `Agent` enum + `agent_supported()` (gated on the single `active_api()`).
    `StateView` (`endpoint_url` + `active_api`, no more `upstream_apis`) is the
    JS↔Rust contract.
  - `main.rs` — Builder, config resolution (`config.json` → `config.toml` seed →
    defaults; loaded values pass through `sanitize_config` — invalid `listen_addr`
    → loopback default, invalid `endpoint_url` → cleared), background proxy
    lifecycle via `ProxyTask { handle, shutdown }` (`spawn(listener, state)` /
    async `stop()` — graceful shutdown waits for the port to be released before a
    restart binds), opens settings on first run, CloseRequested → hide to tray.
    The proxy runs via `proxy_core::serve_with` on a pre-bound listener.
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
cargo test -p proxy-core        # unit + integration (classification, swap, auth, streaming, endpoint)
cargo test -p copilot-proxy     # agent gating + endpoint/listen validation
cargo check --all-targets
cargo clippy --all-targets
cargo tauri dev                 # manual — no config needed; first run opens settings
```

Config lives in `config.json` next to the exe (written by the settings window);
`config.toml` is an optional one-time seed. Version is shared via
`[workspace.package]`; bump it **and** `tauri.conf.json`.
