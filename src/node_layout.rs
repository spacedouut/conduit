// Copyright 2026 spacedouut
//
// SPDX-License-Identifier: GPL-3.0-only

use std::{collections::HashMap, fs, path::PathBuf};

use adw::glib;

#[derive(Default)]
pub struct NodeLayout {
    positions: HashMap<String, (f32, f32)>,
    path: PathBuf,
}

impl NodeLayout {
    pub fn load() -> Self {
        let path = glib::user_config_dir().join("conduit/node-layout");
        let mut layout = Self {
            positions: HashMap::new(),
            path,
        };

        let Ok(contents) = fs::read_to_string(&layout.path) else {
            return layout;
        };

        for line in contents.lines() {
            let mut fields = line.split('\t');
            let (Some(key), Some(x), Some(y)) = (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let (Ok(x), Ok(y)) = (x.parse(), y.parse()) else {
                continue;
            };
            let Some(key) = decode_key(key) else {
                continue;
            };
            layout.positions.insert(key, (x, y));
        }

        layout
    }

    pub fn position(&self, key: &str) -> Option<(f32, f32)> {
        self.positions.get(key).copied()
    }

    pub fn set_position(&mut self, key: String, x: f32, y: f32) {
        self.positions.insert(key, (x, y));
        self.save();
    }

    fn save(&self) {
        let Some(parent) = self.path.parent() else {
            return;
        };
        if let Err(error) = fs::create_dir_all(parent) {
            log::warn!("Could not create node-layout directory: {error}");
            return;
        }

        let mut positions: Vec<_> = self.positions.iter().collect();
        positions.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        let contents = positions
            .into_iter()
            .map(|(key, (x, y))| format!("{}\t{x}\t{y}\n", encode_key(key)))
            .collect::<String>();

        let temporary_path = self.path.with_extension("tmp");
        if let Err(error) = fs::write(&temporary_path, contents)
            .and_then(|_| fs::rename(&temporary_path, &self.path))
        {
            log::warn!("Could not save node layout: {error}");
            let _ = fs::remove_file(temporary_path);
        }
    }
}

pub(crate) fn encode_key(key: &str) -> String {
    key.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn decode_key(encoded: &str) -> Option<String> {
    if encoded.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..encoded.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&encoded[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::{decode_key, encode_key};

    #[test]
    fn node_names_round_trip() {
        let key = "alsa_output.usb-Example Device_123-00.analog-stereo";
        assert_eq!(decode_key(&encode_key(key)).as_deref(), Some(key));
    }
}
