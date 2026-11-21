use std::{fs, path::PathBuf};

use crate::persistence;

pub struct VolumeSliderSetting {
    path: PathBuf,
    show: bool,
}

impl Default for VolumeSliderSetting {
    fn default() -> Self {
        Self::load()
    }
}

impl VolumeSliderSetting {
    pub fn load() -> Self {
        let path = persistence::config_path("show-volume-sliders");
        let show = fs::read_to_string(&path)
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(true);
        Self { path, show }
    }

    pub fn show(&self) -> bool {
        self.show
    }

    pub fn set_show(&mut self, show: bool) {
        self.show = show;
        persistence::write_atomically(&self.path, &show.to_string(), "volume slider preference");
    }
}
