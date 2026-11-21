// Copyright 2026 spacedouut
//
// SPDX-License-Identifier: GPL-3.0-only

//! Helpers shared by the stores that keep user state in the config directory.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use adw::glib;

/// The limit Linux puts on the symlinks followed while resolving a path.
const MAX_LINK_DEPTH: usize = 40;
const MAX_TEMPORARY_FILE_ATTEMPTS: u32 = 100;

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
    let path = &link_target(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let (mut file, temporary_path) = create_temporary_file(path)?;
    let result = file
        .write_all(contents.as_bytes())
        .and_then(|_| fs::rename(&temporary_path, path));
    if result.is_err() {
        let _ = fs::remove_file(temporary_path);
    }
    result
}

/// The file a save of `path` has to replace: `path` itself, or the file it
/// links to, so that a symlinked store keeps its link. The target may not
/// exist yet.
fn link_target(path: &Path) -> io::Result<PathBuf> {
    let mut path = path.to_path_buf();
    for _ in 0..MAX_LINK_DEPTH {
        let Ok(target) = fs::read_link(&path) else {
            return Ok(path);
        };
        path = match path.parent() {
            Some(parent) => parent.join(target),
            None => target,
        };
    }
    Err(io::Error::other("too many levels of symbolic links"))
}

/// Create the file the new contents of `path` are written to before it
/// replaces `path`. Existing files are never reused, so a save cannot write
/// over something it did not create, and a file left behind by an interrupted
/// save does not block later ones.
fn create_temporary_file(path: &Path) -> io::Result<(fs::File, PathBuf)> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut attempt = 0;
    loop {
        let temporary_path =
            path.with_file_name(format!(".{name}.{}-{attempt}.tmp", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((file, temporary_path)),
            Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists
                    && attempt < MAX_TEMPORARY_FILE_ATTEMPTS =>
            {
                attempt += 1
            }
            Err(error) => return Err(error),
        }
    }
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
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
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

    #[cfg(unix)]
    #[test]
    fn write_creates_missing_symlink_target() {
        let directory = test_directory("dangling-symlink");
        let target = directory.join("dotfiles/store");
        let link = directory.join("store");
        std::os::unix::fs::symlink("dotfiles/store", &link).unwrap();

        assert!(write_atomically(&link, "one\n", "test"));
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "one\n");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn write_leaves_neighbouring_files_alone() {
        let directory = test_directory("neighbours");
        let path = directory.join("store");
        fs::write(directory.join("store.tmp"), "theirs\n").unwrap();

        assert!(write_atomically(&path, "ours\n", "test"));
        assert_eq!(
            fs::read_to_string(directory.join("store.tmp")).unwrap(),
            "theirs\n"
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn leftover_temporary_file_does_not_block_saves() {
        let directory = test_directory("leftover");
        let path = directory.join("store");
        let (_, leftover) = create_temporary_file(&path).unwrap();
        fs::write(&leftover, "stale\n").unwrap();

        assert!(write_atomically(&path, "new\n", "test"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert_eq!(fs::read_to_string(&leftover).unwrap(), "stale\n");
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_loop_fails_without_replacing_the_link() {
        let directory = test_directory("symlink-loop");
        let link = directory.join("store");
        std::os::unix::fs::symlink("store", &link).unwrap();

        assert!(!write_atomically(&link, "lost\n", "test"));
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn failed_write_cleans_up_and_keeps_destination() {
        let directory = test_directory("failed-write");
        let path = directory.join("store");
        // A non-empty directory can't be replaced, so the final rename fails.
        fs::create_dir(&path).unwrap();
        fs::write(path.join("kept"), "kept\n").unwrap();

        assert!(!write_atomically(&path, "lost\n", "test"));
        assert_eq!(fs::read_to_string(path.join("kept")).unwrap(), "kept\n");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        let _ = fs::remove_dir_all(directory);
    }
}
