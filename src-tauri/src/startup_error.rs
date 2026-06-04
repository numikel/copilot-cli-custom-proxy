//! Fatal startup error reporting. A missing config is no longer fatal (the app
//! starts with defaults and opens the settings window); this is reserved for
//! genuine failures, e.g. the Tauri runtime failing to start.

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
