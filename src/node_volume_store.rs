// Copyright 2026 spacedouut
//
// SPDX-License-Identifier: GPL-3.0-only

//! Volumes the user set in Conduit, by `node.name`, so they survive a restart.

use std::{collections::HashMap, fs, path::PathBuf};

use crate::persistence::{self, decode as decode_key, encode as encode_key};

#[derive(Default)]
pub struct NodeVolumeStore {
    volumes: HashMap<String, f32>,
    path: PathBuf,
}

impl NodeVolumeStore {
    pub fn load() -> Self {
        let path = persistence::config_path("node-volumes");
        let volumes = fs::read_to_string(&path)
            .map(|contents| parse(&contents))
            .unwrap_or_default();
        Self { volumes, path }
    }

    pub fn volume(&self, key: &str) -> Option<f32> {
        self.volumes.get(key).copied()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.volumes.contains_key(key)
    }

    /// Remember a volume. Returns whether it changed and needs saving.
    pub fn set_volume(&mut self, key: &str, volume: f32) -> bool {
        if !volume.is_finite() {
            return false;
        }
        let volume = volume.max(0.0);
        match self.volumes.get(key) {
            Some(saved) if (saved - volume).abs() <= f32::EPSILON => false,
            _ => {
                self.volumes.insert(key.to_string(), volume);
                true
            }
        }
    }

    pub fn save(&self) {
        persistence::write_atomically(&self.path, &serialize(&self.volumes), "node volumes");
    }
}

fn parse(contents: &str) -> HashMap<String, f32> {
    contents
        .lines()
        .filter_map(|line| {
            let (key, volume) = line.split_once('\t')?;
            let volume: f32 = volume.trim().parse().ok()?;
            (volume.is_finite() && volume >= 0.0).then_some((decode_key(key)?, volume))
        })
        .collect()
}

fn serialize(volumes: &HashMap<String, f32>) -> String {
    let mut volumes: Vec<_> = volumes.iter().collect();
    volumes.sort_unstable_by_key(|(key, _)| *key);
    volumes
        .into_iter()
        .map(|(key, volume)| format!("{}\t{volume}\n", encode_key(key)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volumes_round_trip() {
        let mut store = NodeVolumeStore::default();
        assert!(store.set_volume("alsa_output.pci-0000_00_1f.3.analog-stereo", 0.42));
        assert!(store.set_volume("Firefox", 1.25));
        assert!(store.set_volume("Firefox#2", 0.5));
        assert!(!store.set_volume("Firefox", 1.25));
        let parsed = parse(&serialize(&store.volumes));
        assert_eq!(parsed, store.volumes);
    }

    #[test]
    fn ignores_broken_lines() {
        let contents = format!(
            "zz\t0.5\n{}\tnan\n{}\t0.3\nnot a line\n",
            encode_key("a"),
            encode_key("b")
        );
        let parsed = parse(&contents);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.get("b"), Some(&0.3));
    }

    #[test]
    fn serializes_in_the_saved_format() {
        let volumes = [("b".to_owned(), 0.25), ("a".to_owned(), 1.0)];
        assert_eq!(
            serialize(&volumes.into_iter().collect()),
            "61\t1\n62\t0.25\n"
        );
    }
}
