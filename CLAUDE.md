# CLAUDE.md — copilot-cli-custom-proxy

Local OpenAI-compatible proxy (Windows tray app) that live-swaps the LLM model
for CLI agents (Copilot, Codex) without restarting the terminal.

## Architecture

Cargo workspace, two members:

- **`proxy-core/`** — GUI-independent core (fully testable). Axum reverse proxy
  that rewrites the `model` field and injects the API key.
  - `settings.rs` — `RuntimeConfig { listen_addr, endpoint_url, default_model,
    expose_to_network, proxy_token }` persisted to `config.json` (live source of
    truth). `ApiKind { Chat, Responses }` is **derived from the endpoint URL
    suffix** (`/chat/completions` | `/responses`); `base_url()`/`models_url()`
    strip it. `validate_endpoint_url` (rejects URLs that stop at `/v1` **and**
    URLs with `user:pass@` credentials), `validate_listen_addr` (strict host
    whitelist — RFC 1123 chars or bracketed IPv6 — so the address is safe to
    interpolate into a launched CLI command; port 1–65535).
    `is_loopback_listen_addr` (network-exposure gate) and `generate_proxy_token`
    (random 32-char hex gateway token). `load` distinguishes a missing file
    (silent `None`) from a corrupt one (`warn` + `None`); `save` writes atomically
    (see `atomic_io`).
  - `state.rs` — `AppState` (shared `Arc`): `Mutex<RuntimeConfig>` + `config_path`,
    models `Vec<ModelInfo>`, selected model, in-memory `SecretString` key,
    `RequestLog`, `ui_io: Mutex<()>` (serializes the `ui_state.json`
    read-modify-write). Accessors: `endpoint_url()`, `base_url()`, `models_url()`,
    `active_api()`, `listen_addr()`, `expose_to_network()`, `proxy_token()`,
    `set_endpoint()`, `set_listen_addr()`, `set_expose_to_network()` (mints a
    token on first enable), `regenerate_proxy_token()`, `swap_endpoint()` (atomic
    URL + catalog swap, keeps the selection if it survives — closes the
    empty-`model` race), `models()`, `chat_model_ids()`, `set_models()`. Persists
    config on mutation.
  - `models.rs` — `ModelInfo { id, chat, kind }`, `ModelKind`, and
    `classify_model(id)` (pure, id-heuristic, unit-tested; matches on **word
    tokens**, not raw substrings, so e.g. `watts-3b`/`vanguard-instruct` stay
    chat).
  - `atomic_io.rs` — `write_atomic(path, bytes)` (crate-internal): temp-write +
    `fsync` + `rename`, so an interrupted save can't truncate `config.json` /
    `ui_state.json`. Used by both `save()` methods.
  - `proxy.rs` — Axum router, request/response rewriting, `fetch_models()`
    (uses `models_url()`) and `fetch_models_from(http, endpoint_url, key)` (probes
    a **candidate** endpoint without mutating state — used by `set_endpoint` so the
    catalog can be swapped atomically); forwards to `base_url() + path` (502 if
    unconfigured). `peer_is_authorized` (pure) + `gateway_auth` middleware
    enforce the gateway token for **non-loopback** peers (loopback exempt); the
    layer + `ConnectInfo<SocketAddr>` are wired in `lib.rs::serve_with`, **not**
    `build_router` (tests serve the bare router).
  - `config.rs` — legacy `Config` from `config.toml`; `into_runtime()` migrates it
    to `RuntimeConfig` (seed only; `config.toml` is optional as of 0.3.0).
  - `ui_state.rs` — `ui_state.json` (non-secret prefs) next to the config;
    `AppState` keeps a per-endpoint tray-visible model set (`visible_model_ids()`,
    `set_visible_models()`; `None` = all chat models). `load` logs corruption;
    `save` is atomic. `ui_state.json` is gitignored.
- **`src-tauri/`** — Tauri v2 app.
  - `tray.rs` — native tray menu (status line, **"Models" submenu** of the
    endpoint's visible chat models, Refresh, Run <agent> for supported agents,
    Settings, Quit) + two-state icon (`update_tray_icon`, PNGs `include_bytes!`d).
    Models are in a submenu so first-level items stay reachable with huge catalogs.
  - `commands.rs` — `#[tauri::command]`s: `get_state`, `set_api_key`,
    `forget_api_key`, `set_model`, `run_agent`, `list_agents`, `refresh_models`,
    `set_visible_models`, `set_endpoint`, `set_listen_addr` (both async;
    `set_endpoint` probes + swaps atomically, `set_listen_addr` eagerly binds the
    new address — a bind error surfaces to the UI — and rejects a non-loopback
    address unless `expose_to_network` is on; no-op when unchanged),
    `set_expose_to_network` / `regenerate_proxy_token`. `local_base_url()` maps a
    wildcard/non-loopback listen address to `127.0.0.1:<port>` for the launched
    agents (loopback peer ⇒ no token needed). `Agent` enum + `agent_supported()`
    (gated on the single `active_api()`). `StateView` (`endpoint_url`,
    `active_api`, `expose_to_network`, `proxy_token`) is the JS↔Rust contract.
  - `main.rs` — Builder, config resolution (`config.json` → `config.toml` seed →
    defaults; loaded values pass through `sanitize_config` — invalid `listen_addr`
    → loopback default, invalid `endpoint_url` → cleared, non-loopback addr without
    `expose_to_network` → loopback default, exposed-but-tokenless → token minted),
    background proxy lifecycle via `ProxyTask { handle, shutdown }`
    (`spawn(listener, state)` / async `stop()` — graceful shutdown waits for the
    port to be released before a restart binds), opens settings on first run,
    CloseRequested → hide to tray. The proxy runs via `proxy_core::serve_with` on
    a pre-bound listener.
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
- The **upstream API key** is **in-memory only** — never persist it. The
  **gateway `proxy_token`** is a separate, lower-sensitivity credential that
  protects network-exposed access; it **is** persisted in `config.json` by
  design (a remote device must not re-pair every restart). Don't conflate them.

## Validate

```bash
cargo test -p proxy-core        # unit + integration (classification, atomic_io, gateway auth, swap, streaming, endpoint, loopback)
cargo test -p copilot-proxy     # agent gating, endpoint/listen validation, local_base_url
cargo check --all-targets
cargo clippy --all-targets
cargo tauri dev                 # manual — no config needed; first run opens settings
```

Config lives in `config.json` next to the exe (written by the settings window);
`config.toml` is an optional one-time seed. Version is shared via
`[workspace.package]`; bump it **and** `tauri.conf.json`.
