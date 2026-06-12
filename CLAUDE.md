# CLAUDE.md — copilot-cli-custom-proxy

Local OpenAI-compatible proxy (Windows tray app) that live-swaps the LLM model
for CLI agents (Copilot, Codex) without restarting the terminal.

## Architecture

Cargo workspace, two members:

- **`proxy-core/`** — GUI-independent core (fully testable). Axum reverse proxy
  that rewrites the `model` field and injects the API key.
  - `settings.rs` — `RuntimeConfig { listen_addr, endpoint_url,
    expose_to_network, proxy_token }` persisted to `config.json` (live source of
    truth). The active model is **not** stored here — it is remembered
    per-endpoint in `ui_state.json` (see `ui_state.rs`). `ApiKind { Chat, Responses }` is **derived from the endpoint URL
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
    URL + catalog swap; keeps the selection if it survives, else restores the new
    endpoint's persisted choice, else first — closes the empty-`model` race),
    `models()`, `chat_model_ids()`, `set_models()` (re-applies the persisted
    per-endpoint selection when the in-memory one is empty/stale — this is how the
    active model survives a restart), `set_selected_model()` (persists the choice
    per-endpoint to `ui_state.json`). Persists config on mutation.
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
    to `RuntimeConfig` (seed only; `config.toml` is optional as of 0.3.0). Its
    `default_model` is parsed for back-compat but **no longer forwarded** (the
    active model is now a per-endpoint `ui_state.json` preference).
  - `ui_state.rs` — `ui_state.json` (non-secret prefs) next to the config; two
    per-endpoint maps keyed by endpoint base URL: a tray-visible model set
    (`visible_model_ids()`, `set_visible_models()`; `None` = all chat models) and
    the active-model selection (`selected_models`; restored on catalog load —
    missing = first model). Both writes serialize under `ui_io`. `load` logs
    corruption; `save` is atomic. `ui_state.json` is gitignored.
- **`src-tauri/`** — Tauri v2 app.
  - `tray.rs` — native tray menu (status line, **"Models" submenu** of the
    endpoint's visible chat models, Refresh, Run <agent> for supported agents,
    Settings, Quit) + two-state icon (`update_tray_icon`, PNGs `include_bytes!`d).
    Models are in a submenu so first-level items stay reachable with huge catalogs.
  - `commands.rs` — `#[tauri::command]`s: `get_state`, `set_api_key`,
    `forget_api_key`, `set_model`, `run_agent`, `list_agents`, `refresh_models`,
    `set_visible_models`, `set_endpoint`, `set_listen_addr` (both async;
    `set_endpoint` probes + swaps atomically — and **deliberately does not restart
    the proxy** (routing reads `base_url()` per request; only an in-flight request
    finishes against the old upstream), `set_listen_addr` eagerly binds the
    new address — a bind error surfaces to the UI — and rejects a non-loopback
    address unless `expose_to_network` is on; no-op when unchanged),
    `set_expose_to_network` / `regenerate_proxy_token`. The commands that change
    tray-visible state — `set_api_key`, `forget_api_key`, `set_model` — take
    `AppHandle` and call `tray::apply_menu` so the icon/checkmark stay in sync
    (same as `set_endpoint`/`set_listen_addr`). `local_base_url()` maps a
    wildcard/non-loopback listen address to `127.0.0.1:<port>` for the launched
    agents (loopback peer ⇒ no token needed). `Agent` enum + `agent_supported()`
    (gated on the single `active_api()`). `StateView` (`endpoint_url`,
    `active_api`, `expose_to_network`, `proxy_token`, `running_agent`) is the
    JS↔Rust contract. `AgentWatch` (managed state, `.manage`d in `main.rs`) is a
    deliberately **single-slot** registry of the launched agent terminal: it owns
    the spawned `Child`, and `running_id()` answers from the **live process
    state** (`try_wait`, reaping the slot once the terminal exits) — every
    `get_state` poll re-checks reality, so there is no one-shot exit notification
    to miss and a stale "live" can't outlive the process. A newer launch
    supersedes the previous entry (the superseded terminal keeps running, just
    untracked). Windows-only spawn bits (`creation_flags`) stay
    `#[cfg(windows)]`-gated with a non-Windows counterpart, because CI clippy
    runs on ubuntu.
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
    machine) + `validation.js` + `fonts/` (local IBM Plex woff2).
    `validation.js` holds the **pure** helpers (`detectApi`, `endpointError`,
    `listenAddrError`, …) as a classic script (globals) with a trailing
    `module.exports` guard, so the same file runs in the webview **and** under
    `node --test` (`src-tauri/webview-tests/`, outside `dist/` so it doesn't
    ship). `listenAddrError` mirrors `proxy-core`'s `validate_listen_addr` —
    keep them in sync; JS validation is pre-flight UX only, the backend
    re-validates everything. Webview conventions: long-running actions take an
    in-flight guard (`setRefreshSpinning` / `setEndpointBusy` — flag set before
    the first `await`, cleared in `finally`, early-return also catches the
    Enter path); the `loading`/`error` phases are sticky — a **successful
    action** clears them explicitly, never `adoptState` (the 1.5 s poll would
    mask fresh errors).

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
cargo test -p copilot-proxy     # agent gating, endpoint/listen validation, local_base_url, agent watch
cargo check --all-targets
cargo clippy --all-targets
node --test "src-tauri/webview-tests/*.test.js"   # webview validation helpers (glob form — pointing at the bare dir fails)
cargo tauri dev                 # manual — no config needed; first run opens settings
```

Config lives in `config.json` next to the exe (written by the settings window);
`config.toml` is an optional one-time seed. Version is shared via
`[workspace.package]`; bump it **and** `tauri.conf.json`.

## Release

CI (`.github/workflows/ci.yml`) builds the Windows exe + MSI/NSIS bundles on
**every** push, but uploads them only as run **artifacts** (Actions → run →
Artifacts, ~90-day retention). A GitHub **Release** is published *only* by the
`Attach to GitHub Release` step, which is gated on `refs/tags/v*`. **No tag ⇒ no
release** — pushing commits or merging a PR never publishes one.

To cut a release:

1. Bump `version` in **both** `Cargo.toml` (`[workspace.package]`) and
   `src-tauri/tauri.conf.json` — they **must match the tag**. The release assets
   are named from `tauri.conf.json` (e.g. `Copilot.Proxy_0.3.3_x64-setup.exe`),
   so a mismatch ships mislabeled installers.
2. Merge to `main` and let CI go green.
3. Tag the **`main` tip** (not a feature branch) and push the tag:
   ```bash
   git tag -a vX.Y.Z <main-sha> -m "Release vX.Y.Z"
   git push origin vX.Y.Z
   ```
   The pushed tag triggers a fresh CI run; on success the release appears under
   **Releases** with `copilot-proxy.exe`, `*.msi`, and `*-setup.exe` attached.

Gotchas:
- Tagging only locally does nothing — the workflow fires on the **pushed** tag.
- Tag a commit that already lives on `main`, else you publish feature-branch code.
- Verify with `gh release view vX.Y.Z --json assets` (expect 3 assets).
