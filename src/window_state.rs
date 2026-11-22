// Copyright 2026 spacedouut
//
// SPDX-License-Identifier: GPL-3.0-only

//! The main window's size and maximized state, kept across restarts.

use std::{fs, path::PathBuf};

use crate::persistence;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowGeometry {
    /// Size of the window while it is not maximized.
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

impl WindowGeometry {
    fn parse(text: &str) -> Option<Self> {
        let mut fields = text.trim_end_matches(['\r', '\n']).split('\t');
        let width = fields.next()?.parse().ok()?;
        let height = fields.next()?.parse().ok()?;
        let maximized = fields.next()?.parse().ok()?;
        if fields.next().is_some() || width <= 0 || height <= 0 {
            return None;
        }
        Some(Self {
            width,
            height,
            maximized,
        })
    }

    fn serialize(&self) -> String {
        format!("{}\t{}\t{}\n", self.width, self.height, self.maximized)
    }
}

pub struct WindowState {
    path: PathBuf,
    /// What the file is known to hold.
    saved: Option<WindowGeometry>,
}

impl Default for WindowState {
    fn default() -> Self {
        Self::load(persistence::config_path("window-state"))
    }
}

impl WindowState {
    fn load(path: PathBuf) -> Self {
        let saved = fs::read_to_string(&path)
            .ok()
            .and_then(|text| WindowGeometry::parse(&text));
        Self { path, saved }
    }

    /// The geometry to restore, if a valid one was saved.
    pub fn geometry(&self) -> Option<WindowGeometry> {
        self.saved
    }

    pub fn save(&mut self, geometry: WindowGeometry) {
        if geometry.width <= 0 || geometry.height <= 0 || self.saved == Some(geometry) {
            return;
        }
        if persistence::write_atomically(&self.path, &geometry.serialize(), "window size") {
            self.saved = Some(geometry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEOMETRY: WindowGeometry = WindowGeometry {
        width: 900,
        height: 600,
        maximized: true,
    };

    #[test]
    fn saved_geometry_is_loaded_again() {
        let directory = persistence::test_directory("window-state");
        let path = directory.join("window-state");
        let mut state = WindowState::load(path.clone());
        assert_eq!(state.geometry(), None);

        state.save(GEOMETRY);
        assert_eq!(fs::read_to_string(&path).unwrap(), "900\t600\ttrue\n");
        assert_eq!(WindowState::load(path).geometry(), Some(GEOMETRY));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn unchanged_geometry_is_not_written_again() {
        let directory = persistence::test_directory("window-state-unchanged");
        let path = directory.join("window-state");
        let mut state = WindowState::load(path.clone());
        state.save(GEOMETRY);
        fs::remove_file(&path).unwrap();

        state.save(GEOMETRY);
        assert!(!path.exists());
        state.save(WindowGeometry {
            maximized: false,
            ..GEOMETRY
        });
        assert_eq!(fs::read_to_string(&path).unwrap(), "900\t600\tfalse\n");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn failed_save_is_retried() {
        let directory = persistence::test_directory("window-state-failed");
        let path = directory.join("window-state");
        // A non-empty directory can't be replaced, so the save fails.
        fs::create_dir(&path).unwrap();
        fs::write(path.join("kept"), "").unwrap();
        let mut state = WindowState::load(path.clone());
        state.save(GEOMETRY);
        assert_eq!(state.geometry(), None);

        fs::remove_dir_all(&path).unwrap();
        state.save(GEOMETRY);
        assert_eq!(WindowState::load(path).geometry(), Some(GEOMETRY));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn invalid_geometry_is_ignored() {
        for text in [
            "",
            "900\t600",
            "900\t600\tyes",
            "0\t600\tfalse",
            "900\t-1\tfalse",
            "900\t600\tfalse\textra",
            "wide\t600\tfalse",
            "99999999999\t600\tfalse",
        ] {
            assert_eq!(WindowGeometry::parse(text), None, "{text:?}");
        }
        assert_eq!(
            WindowGeometry::parse("900\t600\tfalse"),
            Some(WindowGeometry {
                maximized: false,
                ..GEOMETRY
            })
        );

        let directory = persistence::test_directory("window-state-invalid");
        let path = directory.join("window-state");
        let mut state = WindowState::load(path.clone());
        state.save(WindowGeometry {
            width: 0,
            ..GEOMETRY
        });
        assert!(!path.exists());
        let _ = fs::remove_dir_all(directory);
    }
}
