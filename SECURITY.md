# Security Policy

## Supported versions

This is an actively developed, pre-1.0 application. Only the latest released
version receives security fixes.

| Version | Supported |
|---------|-----------|
| 0.3.x   | ✅        |
| < 0.3   | ❌        |

## Reporting a vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

**GitHub Private Vulnerability Reporting** (preferred) — open the [Security tab](https://github.com/numikel/copilot-cli-custom-proxy/security) of this repository and click **"Report a vulnerability"**.

Please include:

- a description of the issue and its impact,
- steps to reproduce (a minimal `config.json` / endpoint setup if relevant),
- the version (`copilot-proxy --version` or the tray "About" / settings).

You can expect an initial acknowledgement within a few days. Please give a
reasonable window for a fix before any public disclosure.

## Security model

A few properties are intentional and worth knowing when assessing a report:

- **The API key is in-memory only.** It is wrapped in `secrecy::SecretString`,
  never written to `config.json`, never logged, and never returned to the
  webview. It must be re-entered after restarting the app.
- **The proxy is loopback-only by default** (`127.0.0.1`). Because it injects
  your upstream API key into every forwarded request, binding beyond loopback is
  an explicit opt-in ("expose to network") gated by a self-generated
  **gateway token** required for non-loopback peers.
- **Inputs are validated on entry *and* on load.** The listen address (strict
  host whitelist) and endpoint URL (rejects bare `/v1` and `user:pass@`
  credentials) are validated both in the UI and when `config.json` is read at
  startup; an invalid hand-edited value falls back to a safe default.
- **The settings webview loads only local, static assets** under a restrictive
  `'self'` CSP — no remote scripts, no CDNs.

If you find a way to defeat any of these properties (e.g. key leakage to disk or
logs, an unauthenticated non-loopback request reaching the upstream, or command
injection via a config value), that is in scope — please report it.
