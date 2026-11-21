// Copyright 2026 spacedouut
//
// SPDX-License-Identifier: GPL-3.0-only

use std::{collections::HashMap, fs, path::PathBuf};

use crate::persistence::{self, decode as decode_key, encode as encode_key};

#[derive(Default)]
pub struct NodeLayout {
    positions: HashMap<String, (f32, f32)>,
    path: PathBuf,
}

impl NodeLayout {
    pub fn load() -> Self {
        let path = persistence::config_path("node-layout");
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
        persistence::write_atomically(&self.path, &serialize(&self.positions), "node layout");
    }
}

fn serialize(positions: &HashMap<String, (f32, f32)>) -> String {
    let mut positions: Vec<_> = positions.iter().collect();
    positions.sort_unstable_by_key(|(key, _)| *key);
    positions
        .into_iter()
        .map(|(key, (x, y))| format!("{}\t{x}\t{y}\n", encode_key(key)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{decode_key, encode_key};

    #[test]
    fn node_names_round_trip() {
        let key = "alsa_output.usb-Example Device_123-00.analog-stereo";
        assert_eq!(decode_key(&encode_key(key)).as_deref(), Some(key));
    }

    #[test]
    fn serializes_in_the_saved_format() {
        let positions = [("b".to_owned(), (1.5, -2.0)), ("a".to_owned(), (0.0, 30.0))];
        assert_eq!(
            super::serialize(&positions.into_iter().collect()),
            "61\t0\t30\n62\t1.5\t-2\n"
        );
    }
}
