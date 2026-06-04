# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/), and this project adheres to
[Semantic Versioning](https://semver.org/).

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
