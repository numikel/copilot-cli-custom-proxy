# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/), and this project adheres to
[Semantic Versioning](https://semver.org/).

## [0.3.5] — 2026-06-13

### Fixed
- **Endpoint URLs without a host are rejected up front** — a URL whose API
  suffix is all there is (`https://chat/completions`, `https://responses`) or
  that has no host at all (`https:///responses`) used to pass validation and
  then produce a broken upstream base URL. The suffix is now matched against the
  URL's *path*, and a host is required, so these fail in the settings window with
  a clear message instead of misrouting requests later.
- **A corrupt `config.json` is preserved before it's reset** — if the file can't
  be parsed at startup it is copied to `config.json.bak` *before* anything
  overwrites it, and the settings window shows a one-time notice that the config
  was reset to defaults. Previously a damaged (or briefly unreadable) config was
  silently replaced with no way to recover the original.
- **"Refresh models" keeps your tray-visibility selection** — refreshing the
  catalog (from the tray or the settings window) now re-applies this endpoint's
  saved set of tray-visible models, so the "Models" submenu no longer goes empty
  when the refreshed catalog drops or reorders ids.

### Changed
- **First run writes a default `config.json` immediately** — the file now exists
  from the first launch (next to the executable), so there is something to
  inspect and hand-edit even before you change anything in the settings window.
- **Unknown model kinds degrade gracefully** — a model whose kind isn't one the
  UI knows about now gets a neutral tag (and a console warning) instead of an
  unstyled, nonexistent CSS class.
- Internal: `main.rs` was split into focused `lifecycle` (proxy task) and
  `config_resolve` (startup config) modules; the unused non-atomic endpoint-swap
  path was removed (the atomic swap is the only one); and an internal storage
  type is no longer part of the core crate's public surface.

## [0.3.4] — 2026-06-11

### Added
- **The "live" indicator is now backed by the real agent process** — the app
  tracks the terminal it launches (Copilot/Codex) and reports it in the polled
  state, so the settings window shows "live" while that terminal is open and
  clears within ~1.5 s of closing it. This also works when the agent is started
  from the **tray** (previously only a launch from the settings window lit the
  indicator, and it never went out).
- **Webview unit tests** — the pure validation helpers moved to
  `dist/validation.js` (shared between the webview and Node) and are covered by
  `node --test` in `src-tauri/webview-tests/`, wired into CI.

### Fixed
- **API-key save/forget errors are reported** — a failing `set_api_key` /
  `forget_api_key` call now surfaces as an error toast and leaves the UI state
  untouched, instead of pretending the key was saved/forgotten.
- **Stale error banner clears after a successful endpoint change** — fixing the
  endpoint no longer leaves the previous fetch error on screen until a manual
  Retry.
- **Endpoint apply can no longer double-fire** — while a *Save endpoint* /
  API-switch request is in flight, the controls are disabled and re-submits
  (including Enter) are ignored, so two rapid confirmations can't race each
  other.
- **Listen-address validation matches the backend** — bracketed IPv6 literals
  like `[::1]:8080` are no longer falsely rejected, and the port is checked
  against the real 1–65535 range (`...:0` and `...:70000` now fail fast with
  the backend's message instead of being sent and bounced).
- The copy-able PowerShell command block now falls back to the actual default
  listen address (`127.0.0.1:8080`) instead of a stale port when no address is
  configured yet.

### Changed
- All buttons in the settings window declare `type="button"` (no accidental
  form-submit semantics), and the endpoint / listen-address / API-key / model
  filter inputs have accessible names (`aria-label`) for screen readers.

## [0.3.3] — 2026-06-09

### Fixed
- **Your active model is now remembered** — the model picked in the tray (or the
  settings window) is saved **per-endpoint** in `ui_state.json` and restored on
  the next launch, instead of resetting to the first model in the catalog.
  Switching endpoints back and forth restores each one's own last choice.
- **Tray icon reflects key changes immediately** — setting or forgetting the API
  key now rebuilds the tray, so the icon switches between its ready and idle
  states right away (previously it only updated on the next unrelated tray
  action). Picking a model from the settings window likewise refreshes the tray
  checkmark and icon.

### Changed
- The active-model preference moved out of `config.json` (`default_model`) into
  the per-endpoint `ui_state.json`. A `default_model` left in a legacy
  `config.toml` is parsed but ignored — it is no longer migrated forward.
- Internal: `set_api_key` / `forget_api_key` / `set_model` commands take
  `AppHandle` and refresh the tray; `resolve_config` reuses its directory list
  instead of re-probing the executable path.

## [0.3.2] — 2026-06-09

### Security
- **Network exposure is now opt-in and token-protected** — by default the proxy
  binds to loopback only. Binding beyond `127.0.0.1` requires turning on a new
  *Expose to network* switch in the settings window, which mints a gateway token.
  Once exposed, requests from non-loopback clients must carry that token as
  `Authorization: Bearer <token>` or are rejected with `401`; loopback clients
  (including the locally launched CLI agents) are exempt. The token is shown in
  the UI with copy/regenerate actions and persists in `config.json` (it is a
  self-generated gateway credential, distinct from your upstream API key, which
  stays in memory only). A non-loopback address without the opt-in is reset to
  the loopback default on load.

### Fixed
- **Models with unlucky names are no longer hidden** — model classification now
  matches on word boundaries instead of raw substrings, so ids like `watts-3b`
  or `vanguard-instruct` are correctly treated as chat models rather than being
  filed as audio/moderation and dropped from the tray.
- **Corrupt config/preferences are reported, not silently dropped** — when
  `config.json` or `ui_state.json` is present but unparseable, the reason is now
  logged (a missing file stays silent, as before) so a lost configuration is
  diagnosable.
- **Crash-safe writes** — `config.json` and `ui_state.json` are written to a
  temporary file and atomically renamed into place, so an interrupted write
  (crash, forced kill, power loss) can no longer leave a truncated file.
- **No lost updates to tray-visibility preferences** — the read-modify-write of
  `ui_state.json` is serialized, so concurrent saves can't clobber each other.

### Changed
- The launched CLI agents always target the proxy via `127.0.0.1` (using the
  configured port), even when the proxy is bound to a wildcard/LAN address — so
  the local launch path keeps working without the gateway token.
- `proxy-core` adds `is_loopback_listen_addr`, `generate_proxy_token`, and the
  `expose_to_network` / `proxy_token` runtime-config fields; the served router
  now enforces a peer-aware gateway-auth layer. New Tauri commands
  `set_expose_to_network` and `regenerate_proxy_token`.

## [0.3.1] — 2026-06-09

### Security
- **Strict listen-address validation** — the host part of the listen address is
  now restricted to a conservative character set (letters, digits, `-`, `.`, and
  bracketed IPv6 literals), and the port must be 1–65535. This prevents shell
  metacharacters from reaching the launched CLI's command line. As
  defence-in-depth, the proxy base URL passed to the Codex launcher is wrapped in
  a quoted string (and rejected outright if it contains a quote).
- **Endpoint URLs may no longer embed credentials** — a `user:pass@host` authority
  is rejected, so a key accidentally pasted into the URL can't be persisted to
  `config.json` or written to logs.
- **Config is re-validated on startup** — a hand-edited or swapped-out
  `config.json` is sanitized when loaded: an invalid listen address falls back to
  the loopback default and an invalid endpoint URL is cleared (opening setup),
  rather than being trusted blindly.

### Fixed
- **No more empty `model` during an endpoint change** — switching the endpoint
  now fetches the new catalog *before* swapping, then replaces the URL and model
  list atomically. Previously a request landing mid-switch could be forwarded with
  an empty model field. The current selection is preserved when it still exists in
  the new catalog.
- **Reliable proxy restart on listen-address change** — the background server is
  now shut down gracefully (releasing its port) before the replacement binds, and
  the new address is bound up front so a failure (e.g. the port is already in use)
  is reported in the UI instead of silently leaving the proxy down. Re-confirming
  the same address is a no-op.

### Changed
- `proxy-core` adds `fetch_models_from` (probe a candidate endpoint without
  mutating state), `AppState::swap_endpoint` (atomic URL + catalog swap), and
  `serve_with` (run on a pre-bound listener with graceful shutdown). `serve`
  becomes a thin wrapper over `serve_with`.
- `set_listen_addr` is now async (binds the new address before restarting).

## [0.3.0] — 2026-06-04

### Added
- **In-app configuration** — the endpoint and the local listen address are now
  set in the settings window (no `config.toml` editing required) and persist to
  `config.json` next to the executable (`set_endpoint`, `set_listen_addr`
  commands). The API key remains in memory only.
- **Full-URL endpoint model with a chat ⟷ responses switch** — you enter the
  complete upstream URL (e.g. `https://openrouter.ai/api/v1/responses`); the wire
  API is **derived from the URL suffix** rather than declared separately. A
  single-active switch in the settings window flips the suffix between
  `/chat/completions` and `/responses`. Stopping the URL at `/v1` is rejected
  (the API type would be ambiguous). New `proxy-core` module `settings.rs`
  (`RuntimeConfig`, `ApiKind`, `validate_endpoint_url`, `validate_listen_addr`).
- **Graceful first run** — a missing config no longer shows an error and exits;
  the app starts with defaults and opens the settings window so you can configure
  the endpoint.
- **Live listen-address changes** — changing the address restarts only the
  background proxy task (abort + respawn), without restarting the app or terminal.

### Changed
- `AppState` now holds a mutable `RuntimeConfig` (endpoint URL + listen address +
  default model) with accessors (`endpoint_url()`, `base_url()`, `models_url()`,
  `active_api()`, `listen_addr()`) and persistence via `set_config_path`.
- Agent gating is now based on the single **active** API (derived from the
  endpoint URL): exactly one of Copilot/Codex is enabled at a time.
- `StateView` drops `corporate_base_url` / `upstream_apis`; adds `endpoint_url`
  and `active_api`.
- `config.toml` is now an **optional one-time seed**: on first run it is migrated
  into `config.json` (base URL + first API → full endpoint URL), then ignored.

### Migration
- Existing `config.toml` installs keep working: the first 0.3.0 launch migrates
  them to `config.json`. `config.json` is gitignored.

## [0.2.0] — 2026-06-04

### Added
- **Redesigned settings window** recreated 1:1 from the design handoff: a
  frameless (`decorations: false`) window with a custom title bar (drag region +
  minimize / maximize / close), status pill, and toast notifications.
- **Dark / light theme toggle** (defaults to dark; persisted in `localStorage`).
- **Model classification** in `proxy-core`: `fetch_models` now returns
  `ModelInfo { id, chat, kind }`, inferring non-chat families (`embed`, `image`,
  `audio`, `rerank`, `moderation`) from the model id. The settings window can
  **hide non-chat models** (on by default) and tags the rest by kind.
- **Two-state tray icon** — accent-filled when the proxy is ready, muted outline
  when idle.
- **Tray "Models" submenu** — models moved off the first level so
  "Open Settings…"/"Quit" stay reachable with large catalogs (e.g. OpenRouter).
- **Per-endpoint tray-visibility curation** — pick which chat models appear in
  the tray submenu (checkbox per model, all/none, shift-click range), persisted
  to `ui_state.json` keyed by endpoint (`set_visible_models` command).
- `forget_api_key` command to clear the in-memory key from the UI.
- IBM Plex Sans/Mono fonts bundled locally (no CDN; keeps the `'self'` CSP).

### Changed
- `AppState` now stores `Vec<ModelInfo>` instead of `Vec<String>`; added
  `model_ids()` and `chat_model_ids()` helpers.
- Settings window narrowed to 444 px to match the design; status values are now
  polled live (~1.5 s) from the real `request_log` (no simulated traffic).
- `tauri` built with the `image-png` feature for runtime tray-icon swapping;
  window capabilities extended with `minimize` / `toggle-maximize` / `close` /
  `start-dragging` permissions.

### Notes
- Launchable agents remain Copilot and Codex, but the agent enum / gating / UI
  are structured so additional agents are a single match-arm to add.

## [0.1.0]

- Initial release: tray proxy with live model switching (Rust + Tauri v2),
  Copilot and Codex launchers, in-memory API key, live request logging.
