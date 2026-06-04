use std::path::PathBuf;

/// Failure to load `config.toml` before the Tauri app starts.
#[derive(Debug)]
pub enum ConfigLoadError {
    NotFound { candidates: Vec<PathBuf> },
    Invalid { path: PathBuf, message: String },
}

impl ConfigLoadError {
    pub fn user_message(&self) -> String {
        match self {
            ConfigLoadError::NotFound { candidates } => {
                let paths = candidates
                    .iter()
                    .map(|p| format!("  • {}", p.display()))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "Configuration file config.toml was not found.\n\n\
                     Copy config.example.toml to config.toml next to the executable \
                     (or in the working directory), then edit it.\n\n\
                     Checked paths:\n{paths}"
                )
            }
            ConfigLoadError::Invalid { path, message } => {
                format!(
                    "Could not load config.toml:\n\n{message}\n\nFile: {}",
                    path.display()
                )
            }
        }
    }
}

/// Shows a native error dialog on Windows; prints to stderr elsewhere.
pub fn show_startup_error(title: &str, message: &str) {
    tracing::error!("{message}");

    #[cfg(windows)]
    show_windows_message_box(title, message);

    #[cfg(not(windows))]
    {
        eprintln!("{title}\n\n{message}");
    }
}

#[cfg(windows)]
fn show_windows_message_box(title: &str, message: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(Some(0)).collect()
    }

    let title = wide(title);
    let message = wide(message);
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub fn exit_with_startup_error(err: ConfigLoadError) -> ! {
    let message = err.user_message();
    show_startup_error("Copilot Proxy", &message);
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_message_lists_checked_paths() {
        let err = ConfigLoadError::NotFound {
            candidates: vec![PathBuf::from("C:\\app\\config.toml")],
        };
        let msg = err.user_message();
        assert!(msg.contains("config.toml was not found"));
        assert!(msg.contains("config.example.toml"));
        assert!(msg.contains("C:\\app\\config.toml"));
    }
}
