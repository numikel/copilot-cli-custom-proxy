# Contributing

Thanks for your interest in improving **Copilot CLI Custom Proxy**! This is a
small Windows tray app, but contributions — bug reports, fixes, and features —
are welcome.

## Project layout

This is a Cargo workspace with two members:

- **`proxy-core/`** — the GUI-independent core (Axum reverse proxy, model swap,
  streaming). Fully testable without a GUI.
- **`src-tauri/`** — the Tauri v2 app: tray menu, settings webview, background
  server lifecycle.

See [`CLAUDE.md`](CLAUDE.md) for a deeper architecture tour.

## Development setup

Requirements:

- [Rust](https://rustup.rs) (stable toolchain, edition 2021)
- The [Tauri v2 system prerequisites](https://tauri.app/start/prerequisites/)
  (WebView2 ships with Windows 10/11)
- The Tauri CLI: `cargo install tauri-cli --version "^2.0"`

```powershell
cargo tauri dev   # run the app with a console + logs; first run opens settings
```

No `config.toml` is needed — the app starts with defaults and is configured in
the settings window.

## Before you open a pull request

Run the full local check suite — CI runs the same gates and will reject a PR
that does not pass them:

```bash
cargo fmt --all -- --check          # formatting (CI gate)
cargo clippy --all-targets -- -D warnings   # lints, warnings-as-errors (CI gate)
cargo test -p proxy-core            # core unit + integration tests
cargo check -p copilot-proxy        # type-check the Tauri app
```

To auto-fix formatting before committing: `cargo fmt --all`.

## Conventions

- **Formatting & lints** — code must be `rustfmt`-clean and pass
  `clippy -D warnings`. The repo ships a `rustfmt.toml` and an `.editorconfig`.
- **Frontend is vanilla JS** — `src-tauri/dist/` uses no bundler and no ES
  `import`. Use `window.__TAURI__.core.invoke`. The CSP is `'self'`; no CDNs
  (fonts are bundled in `dist/fonts/`).
- **Never persist the API key.** It lives in memory only
  (`secrecy::SecretString`) — never write it to disk or logs.
- **Tests live with the core.** New behaviour in `proxy-core` should come with a
  test (`proxy-core/tests/` for integration, `#[cfg(test)]` modules for units).
- **Versioning** — the version is shared via `[workspace.package]`. Bump it
  **and** `src-tauri/tauri.conf.json` together, and add a `CHANGELOG.md` entry.

## Branches & commits

- Branch from `main` using a descriptive prefix: `feat/…`, `fix/…`, `chore/…`,
  `docs/…`.
- Keep commits focused; write imperative subject lines (e.g.
  "Reject userinfo in endpoint URL").
- Reference any related issue in the PR description.

## Reporting bugs / requesting features

Use the [issue templates](.github/ISSUE_TEMPLATE). For anything
security-sensitive, follow [`SECURITY.md`](SECURITY.md) instead of opening a
public issue.
