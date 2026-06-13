//! Startup configuration resolution: locates and loads `config.json` (with a
//! legacy `config.toml` seed fallback), sanitizes the loaded values, backs up
//! a corrupt `config.json` before resetting it, and reports whether first-run
//! setup is needed.

use proxy_core::{Config, RuntimeConfig};
use std::path::{Path, PathBuf};

/// Outcome of startup config resolution (see [`resolve_config`]).
pub(crate) struct ResolvedConfig {
    pub config: RuntimeConfig,
    /// Directory the resolved `config.json` lives in (or will be written to).
    pub dir: PathBuf,
    /// Whether the app needs first-run setup (no usable endpoint yet → show
    /// the settings window instead of a silent, idle tray icon).
    pub needs_setup: bool,
    /// Set when a corrupt config.json was reset; surfaced once in the settings window.
    pub startup_warning: Option<String>,
}

/// Directories searched for `config.json` / `config.toml` / `ui_state.json`:
/// next to the executable, then the current working directory.
pub(crate) fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
        }
    }
    dirs.push(PathBuf::from("."));
    dirs
}

/// Re-validates a config loaded from disk and falls back to safe values for any
/// field that fails. `config.json` can be hand-edited (or swapped out), so a
/// malformed `listen_addr` must never reach the agent launcher (command
/// injection) and a malformed `endpoint_url` must not silently misroute or leak
/// credentials. Invalid `listen_addr` → loopback default; invalid
/// `endpoint_url` → cleared (forces the settings window on first run).
pub(crate) fn sanitize_config(mut cfg: RuntimeConfig) -> RuntimeConfig {
    if let Err(e) = proxy_core::validate_listen_addr(&cfg.listen_addr) {
        tracing::warn!(
            addr = %cfg.listen_addr,
            "invalid listen_addr in config ({e}) — falling back to {}",
            proxy_core::DEFAULT_LISTEN_ADDR
        );
        cfg.listen_addr = proxy_core::DEFAULT_LISTEN_ADDR.to_string();
    }
    // A non-loopback bind must never come up without the explicit exposure
    // opt-in (which gates the gateway token) — a hand-edited config that sets a
    // LAN address but not the flag is reset to loopback.
    if !proxy_core::is_loopback_listen_addr(&cfg.listen_addr) && !cfg.expose_to_network {
        tracing::warn!(
            addr = %cfg.listen_addr,
            "non-loopback listen_addr without expose_to_network — falling back to {}",
            proxy_core::DEFAULT_LISTEN_ADDR
        );
        cfg.listen_addr = proxy_core::DEFAULT_LISTEN_ADDR.to_string();
    }
    // An exposed proxy is never tokenless: mint one if the config enabled
    // exposure but carries no token (e.g. hand-edited).
    if cfg.expose_to_network && cfg.proxy_token.as_deref().unwrap_or("").is_empty() {
        cfg.proxy_token = Some(proxy_core::generate_proxy_token());
    }
    if !cfg.endpoint_url.is_empty() {
        if let Err(e) = proxy_core::validate_endpoint_url(&cfg.endpoint_url) {
            tracing::warn!(
                url = %cfg.endpoint_url,
                "invalid endpoint_url in config ({e}) — clearing it"
            );
            cfg.endpoint_url = String::new();
        }
    }
    cfg
}

/// Resolves the runtime config at startup:
/// 1. an existing `config.json` (the source of truth), else
/// 2. a legacy `config.toml`, migrated and seeded into `config.json`, else
/// 3. built-in defaults, seeded into `config.json` (best effort) so the user
///    finds a file to inspect / hand-edit from the first run.
///
/// A *corrupt* `config.json` is backed up to `config.json.bak` before
/// resolution continues — the seeded defaults then replace the broken file in
/// place — and the reset is reported once in the settings window via
/// [`ResolvedConfig::startup_warning`].
///
/// Loaded values are sanitized (see [`sanitize_config`]) **in memory only**:
/// the file on disk is never rewritten with the sanitized values, so it
/// remains the user's source of truth.
pub(crate) fn resolve_config() -> ResolvedConfig {
    resolve_config_in(&candidate_dirs())
}

/// [`resolve_config`] with the searched directories injected, so tests can
/// resolve against temp dirs instead of probing `current_exe()` / the cwd.
fn resolve_config_in(dirs: &[PathBuf]) -> ResolvedConfig {
    let mut startup_warning = None;
    let mut corrupt_dir: Option<PathBuf> = None;

    for dir in dirs {
        let json = dir.join("config.json");
        if !json.exists() {
            continue;
        }
        if let Some(cfg) = RuntimeConfig::load(&json) {
            tracing::info!("loaded config.json from {}", json.display());
            let config = sanitize_config(cfg);
            let needs_setup = !config.is_configured();
            return ResolvedConfig {
                config,
                dir: dir.clone(),
                needs_setup,
                startup_warning,
            };
        }
        // Present but unloadable (`load` already logged the cause). Preserve
        // the original *before* any further resolution can overwrite it: the
        // defaults seeded below replace the corrupt file in place. Best
        // effort — a failed backup must not block startup.
        if startup_warning.is_none() {
            let bak = json.with_extension("json.bak");
            if let Err(e) = std::fs::copy(&json, &bak) {
                tracing::warn!("failed to back up corrupt {}: {e}", json.display());
            } else {
                tracing::warn!(
                    "config.json at {} is corrupt — backed up to {}",
                    json.display(),
                    bak.display()
                );
            }
            startup_warning = Some(
                "config.json was corrupt and has been reset to defaults — \
                 the previous file was saved as config.json.bak"
                    .to_string(),
            );
            corrupt_dir = Some(dir.clone());
        }
    }

    for dir in dirs {
        let toml = dir.join("config.toml");
        if toml.exists() {
            match Config::load(&toml) {
                Ok(legacy) => {
                    let config = sanitize_config(legacy.into_runtime());
                    let json = dir.join("config.json");
                    if let Err(e) = config.save(&json) {
                        tracing::warn!("failed to seed config.json: {e}");
                    } else {
                        tracing::info!("migrated {} → {}", toml.display(), json.display());
                    }
                    let needs_setup = !config.is_configured();
                    return ResolvedConfig {
                        config,
                        dir: dir.clone(),
                        needs_setup,
                        startup_warning,
                    };
                }
                Err(e) => {
                    tracing::warn!(
                        "ignoring unparseable config.toml at {}: {e}",
                        toml.display()
                    )
                }
            }
        }
    }

    // Prefer the directory that held the corrupt config — the defaults seeded
    // below then replace the broken file (its original is already preserved in
    // `config.json.bak`) — else the first candidate.
    let dir = corrupt_dir
        .or_else(|| dirs.first().cloned())
        .unwrap_or_else(|| PathBuf::from("."));
    tracing::info!("no usable config found — starting with defaults (first-run setup)");
    let config = RuntimeConfig::default();
    // Seed the defaults so a `config.json` exists from the first run (and so a
    // corrupt one is replaced). Best effort: the directory may not be writable
    // (e.g. Program Files) — the settings window will retry on first save.
    let json = dir.join("config.json");
    match config.save(&json) {
        Ok(()) => tracing::info!("seeded default config.json at {}", json.display()),
        Err(e) => tracing::warn!(
            "failed to seed default config.json at {}: {e}",
            json.display()
        ),
    }
    ResolvedConfig {
        config,
        dir,
        needs_setup: true,
        startup_warning,
    }
}

pub(crate) fn ui_state_path(dir: &Path) -> PathBuf {
    dir.join("ui_state.json")
}

pub(crate) fn config_json_path(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh, empty per-test directory under the system temp dir (the same
    /// convention as proxy-core's settings tests, but a subdirectory so each
    /// test owns a whole candidate dir).
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("copilot_proxy_resolve_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn corrupt_config_is_backed_up_before_reset() {
        let dir = temp_dir("corrupt_backup");
        let json = dir.join("config.json");
        let garbage = b"{not json";
        std::fs::write(&json, garbage).unwrap();

        let resolved = resolve_config_in(std::slice::from_ref(&dir));

        // The original bytes survive in config.json.bak…
        let bak = dir.join("config.json.bak");
        assert_eq!(std::fs::read(&bak).unwrap(), garbage);
        // …the reset is surfaced to the settings window…
        assert!(resolved.startup_warning.is_some());
        // …and the app comes up on defaults, with the corrupt file replaced by
        // a parseable seeded one.
        let defaults = RuntimeConfig::default();
        assert_eq!(resolved.config.listen_addr, defaults.listen_addr);
        assert_eq!(resolved.config.endpoint_url, defaults.endpoint_url);
        assert_eq!(
            resolved.config.expose_to_network,
            defaults.expose_to_network
        );
        assert_eq!(resolved.config.proxy_token, defaults.proxy_token);
        assert!(resolved.needs_setup);
        assert!(RuntimeConfig::load(&json).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn valid_config_produces_no_warning_and_no_bak() {
        let dir = temp_dir("valid_no_warning");
        let cfg = RuntimeConfig {
            endpoint_url: "https://e.example/v1/responses".to_string(),
            ..RuntimeConfig::default()
        };
        cfg.save(&dir.join("config.json")).unwrap();

        let resolved = resolve_config_in(std::slice::from_ref(&dir));

        assert!(!dir.join("config.json.bak").exists());
        assert!(resolved.startup_warning.is_none());
        assert_eq!(resolved.config.endpoint_url, cfg.endpoint_url);
        assert_eq!(resolved.config.listen_addr, cfg.listen_addr);
        assert!(!resolved.needs_setup);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_run_seeds_default_config_json() {
        let dir = temp_dir("first_run_seed");

        let resolved = resolve_config_in(std::slice::from_ref(&dir));

        let seeded = RuntimeConfig::load(&dir.join("config.json"))
            .expect("first run should seed a parseable config.json");
        assert_eq!(seeded.listen_addr, proxy_core::DEFAULT_LISTEN_ADDR);
        assert!(seeded.endpoint_url.is_empty());
        assert!(resolved.needs_setup);
        assert!(resolved.startup_warning.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_valid_config_is_not_rewritten() {
        let dir = temp_dir("valid_not_rewritten");
        let json = dir.join("config.json");
        // Hand-formatted JSON that `save` would pretty-print differently —
        // byte-for-byte identity after resolution proves nothing rewrote it
        // (sanitization stays in memory; the file remains the source of truth).
        let original =
            b"{\"listen_addr\":\"127.0.0.1:8080\",\"endpoint_url\":\"https://e.example/v1/chat/completions\"}";
        std::fs::write(&json, original).unwrap();

        let resolved = resolve_config_in(std::slice::from_ref(&dir));

        assert_eq!(std::fs::read(&json).unwrap(), original);
        assert!(!resolved.needs_setup);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
