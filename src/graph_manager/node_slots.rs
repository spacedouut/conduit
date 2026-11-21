// SPDX-License-Identifier: GPL-3.0-only

use std::{collections::HashMap, fs, path::PathBuf};

use crate::node_layout::{decode_key, encode_key};

#[derive(Default)]
pub(super) struct NodeSlots {
    pub entries: HashMap<u64, (String, String)>,
    cookie: u32,
    path: Option<PathBuf>,
}

impl NodeSlots {
    pub fn load(cookie: u32) -> Self {
        let path = adw::glib::user_config_dir().join("conduit/node-slots");
        let mut slots = Self::parse(&fs::read_to_string(&path).unwrap_or_default(), cookie);
        slots.path = Some(path);
        slots
    }

    fn parse(contents: &str, cookie: u32) -> Self {
        let mut slots = Self {
            cookie,
            ..Self::default()
        };
        let mut lines = contents.lines();
        if lines.next().and_then(|line| line.parse().ok()) != Some(cookie) {
            return slots;
        }
        for line in lines {
            let fields: Vec<_> = line.split('\t').collect();
            let [serial, base, key] = fields.as_slice() else {
                continue;
            };
            if let (Ok(serial), Some(base), Some(key)) =
                (serial.parse(), decode_key(base), decode_key(key))
            {
                slots.entries.insert(serial, (base, key));
            }
        }
        slots
    }

    fn serialize(&self) -> String {
        let mut contents = format!("{}\n", self.cookie);
        let mut entries: Vec<_> = self.entries.iter().collect();
        entries.sort_unstable_by_key(|(serial, _)| *serial);
        for (serial, (base, key)) in entries {
            contents.push_str(&format!(
                "{serial}\t{}\t{}\n",
                encode_key(base),
                encode_key(key)
            ));
        }
        contents
    }

    pub fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let Some(parent) = path.parent() else {
            return;
        };
        let temporary = path.with_extension("tmp");
        if let Err(error) = fs::create_dir_all(parent)
            .and_then(|_| fs::write(&temporary, self.serialize()))
            .and_then(|_| fs::rename(&temporary, path))
        {
            log::warn!("Could not save node slots: {error}");
            let _ = fs::remove_file(temporary);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_round_trip_and_cookie_change_discards_them() {
        let mut slots = NodeSlots {
            cookie: 123,
            ..NodeSlots::default()
        };
        slots
            .entries
            .insert(42, ("Firefox".into(), "Firefox#2".into()));
        let contents = slots.serialize();
        assert_eq!(NodeSlots::parse(&contents, 123).entries, slots.entries);
        assert!(NodeSlots::parse(&contents, 124).entries.is_empty());
        assert!(NodeSlots::parse("", 123).entries.is_empty());
        assert!(NodeSlots::parse("123\nbad\n42\tzz\t00\n", 123)
            .entries
            .is_empty());
    }
}
