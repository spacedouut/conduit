use std::{fs, path::PathBuf};

use adw::glib;

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
        let path = glib::user_config_dir().join("conduit/show-volume-sliders");
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
        if let Some(parent) = self.path.parent() {
            if let Err(error) =
                fs::create_dir_all(parent).and_then(|_| fs::write(&self.path, show.to_string()))
            {
                log::warn!("Could not save volume slider preference: {error}");
            }
        }
    }
}
