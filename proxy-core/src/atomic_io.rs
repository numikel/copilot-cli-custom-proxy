//! Crash-safe file writes for the small JSON config/preference files.
//!
//! A plain `fs::write` truncates the target up front, so a crash (or
//! `taskkill /F`, or power loss) mid-write leaves a half-written file that the
//! loaders then silently discard. Writing to a sibling temp file and renaming
//! it over the target makes the swap atomic on the same volume: a reader always
//! sees either the complete old file or the complete new one, never a torn one.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

/// Writes `bytes` to `path` atomically: stage into `path` + `.tmp`, flush it to
/// disk, then rename over the target (`fs::rename` replaces an existing file on
/// Windows). The temp file is removed on any failure so a crashed write never
/// leaves stray `.tmp` litter.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // Sibling temp file → same directory, hence same volume, so the rename is a
    // metadata-only atomic swap rather than a cross-device copy.
    let tmp = tmp_path(path);

    if let Err(e) = stage(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Writes the bytes to the temp file and fsyncs them before the rename, so the
/// data is durable on disk by the time the target name points at it.
fn stage(tmp: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// The staging path for `path`: its filename with a `.tmp` suffix appended
/// (kept in the same directory as `path`).
fn tmp_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overwrites_existing_and_leaves_no_temp() {
        let path = std::env::temp_dir().join("copilot_proxy_atomic_io_test.json");
        let _ = std::fs::remove_file(&path);

        write_atomic(&path, b"first").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");

        // A second write replaces the target in place.
        write_atomic(&path, b"second-longer").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second-longer");

        // No `.tmp` sibling is left behind after a successful write.
        assert!(!tmp_path(&path).exists());

        let _ = std::fs::remove_file(&path);
    }
}
