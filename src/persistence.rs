// Copyright 2026 spacedouut
//
// SPDX-License-Identifier: GPL-3.0-only

//! Helpers shared by the stores that keep user state in the config directory.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use adw::glib;

/// Path of the saved-data file `name` in Conduit's config directory.
pub(crate) fn config_path(name: &str) -> PathBuf {
    glib::user_config_dir().join("conduit").join(name)
}

/// Hex-encode a value so it cannot contain the tab and newline separators.
pub(crate) fn encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn decode(encoded: &str) -> Option<String> {
    if encoded.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..encoded.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(encoded.get(index..index + 2)?, 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

/// Replace the file at `path` with `contents`, logging a warning naming
/// `what` on failure. The previous file stays intact unless the write succeeds.
/// Returns whether the file was written.
pub(crate) fn write_atomically(path: &Path, contents: &str, what: &str) -> bool {
    match try_write_atomically(path, contents) {
        Ok(()) => true,
        Err(error) => {
            log::warn!("Could not save {what}: {error}");
            false
        }
    }
}

fn try_write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    // Write through a symlinked file instead of replacing the link.
    let target = fs::canonicalize(path);
    let path = target.as_deref().unwrap_or(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary_path = path.with_extension("tmp");
    let result =
        fs::write(&temporary_path, contents).and_then(|_| fs::rename(&temporary_path, path));
    if result.is_err() {
        let _ = fs::remove_file(temporary_path);
    }
    result
}

/// A fresh directory for a test's files.
#[cfg(test)]
pub(crate) fn test_directory(name: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("conduit-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_matches_saved_files() {
        assert_eq!(encode("hi\nthere"), "68690a7468657265");
        assert_eq!(decode("68690a7468657265").as_deref(), Some("hi\nthere"));
        assert_eq!(encode(""), "");
        assert_eq!(decode("").as_deref(), Some(""));
    }

    #[test]
    fn decode_rejects_malformed_input() {
        assert_eq!(decode("abc"), None);
        assert_eq!(decode("zz"), None);
        assert_eq!(decode("ff"), None);
        assert_eq!(decode("é"), None);
    }

    #[test]
    fn write_creates_directories_and_replaces_contents() {
        let directory = test_directory("write");
        let path = directory.join("nested/store");
        write_atomically(&path, "one\n", "test");
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\n");
        write_atomically(&path, "two\n", "test");
        assert_eq!(fs::read_to_string(&path).unwrap(), "two\n");
        assert!(!path.with_extension("tmp").exists());
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[test]
    fn write_follows_symlinks() {
        let directory = test_directory("symlink");
        let target = directory.join("dotfiles/store");
        write_atomically(&target, "one\n", "test");
        let link = directory.join("store");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(write_atomically(&link, "two\n", "test"));
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "two\n");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn failed_write_keeps_previous_file() {
        let directory = test_directory("failed-write");
        let path = directory.join("store");
        assert!(write_atomically(&path, "kept\n", "test"));
        // A directory in the temporary file's place makes the write fail.
        fs::create_dir(path.with_extension("tmp")).unwrap();
        assert!(!write_atomically(&path, "lost\n", "test"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "kept\n");
        let _ = fs::remove_dir_all(directory);
    }
}
