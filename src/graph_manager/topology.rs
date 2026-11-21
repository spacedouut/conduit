// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{HashMap, HashSet};

use pipewire::spa::utils::Direction;

use super::node_slots::NodeSlots;

#[derive(Clone)]
pub(super) struct PortInfo {
    pub node_id: u32,
    pub name: String,
    pub direction: Direction,
    pub is_monitor: bool,
    pub is_meter: bool,
}

#[derive(Clone)]
pub(super) struct NodeInfo {
    pub serial: Option<u64>,
    pub node_name: String,
    /// Live-unique `node.name`, used to remember layout, volume and links.
    pub key: String,
    pub is_meter: bool,
    /// An application stream, routed by the session manager.
    pub is_stream: bool,
}

/// What the GTK thread knows about the PipeWire nodes and ports on screen.
#[derive(Default)]
pub(super) struct Topology {
    nodes: HashMap<u32, NodeInfo>,
    ports: HashMap<u32, PortInfo>,
    slots: NodeSlots,
    enumerating: bool,
    reservation_displaced: HashSet<u32>,
}

impl Topology {
    pub fn begin_enumeration(&mut self, cookie: u32) {
        self.slots = NodeSlots::load(cookie);
        self.enumerating = true;
    }

    pub fn finish_enumeration(&mut self) -> Vec<(u32, String, String)> {
        self.enumerating = false;
        self.slots
            .entries
            .retain(|serial, _| self.nodes.values().any(|node| node.serial == Some(*serial)));
        let mut displaced: Vec<_> = self.reservation_displaced.drain().collect();
        displaced.sort_unstable_by_key(|id| (self.nodes.get(id).and_then(|node| node.serial), *id));
        let mut occupied: HashSet<_> = self
            .nodes
            .iter()
            .filter(|(id, _)| !displaced.contains(id))
            .map(|(_, node)| node.key.clone())
            .collect();
        let mut changes = Vec::new();
        for id in displaced {
            let Some(node) = self.nodes.get_mut(&id) else {
                continue;
            };
            let mut key = node.node_name.clone();
            let mut suffix = 2;
            while occupied.contains(&key) {
                key = format!("{}#{suffix}", node.node_name);
                suffix += 1;
            }
            occupied.insert(key.clone());
            if key != node.key {
                changes.push((
                    id,
                    std::mem::replace(&mut node.key, key.clone()),
                    key.clone(),
                ));
                if let Some(serial) = node.serial {
                    self.slots
                        .entries
                        .insert(serial, (node.node_name.clone(), key));
                }
            }
        }
        self.slots.save();
        changes
    }

    pub fn add_node(&mut self, id: u32, mut info: NodeInfo) -> String {
        let base = info.key.clone();
        let occupied =
            |key: &str| {
                self.nodes
                    .iter()
                    .any(|(node_id, node)| *node_id != id && node.key == key)
                    || (self.enumerating
                        && self.slots.entries.iter().any(|(serial, (_, saved))| {
                            Some(*serial) != info.serial && saved == key
                        }))
            };
        if let Some((saved_base, key)) = info
            .serial
            .and_then(|serial| self.slots.entries.get(&serial))
        {
            if saved_base == &base && !occupied(key) {
                info.key = key.clone();
            }
        }
        let mut suffix = 2;
        let mut displaced = false;
        while occupied(&info.key) {
            if !self
                .nodes
                .iter()
                .any(|(node_id, node)| *node_id != id && node.key == info.key)
            {
                displaced = true;
            }
            info.key = format!("{base}#{suffix}");
            suffix += 1;
        }
        if displaced {
            self.reservation_displaced.insert(id);
        }
        let key = info.key.clone();
        if let Some(serial) = info.serial {
            self.slots.entries.insert(serial, (base, key.clone()));
            self.slots.save();
        }
        self.nodes.insert(id, info);
        key
    }

    pub fn remove_node(&mut self, id: u32) {
        self.reservation_displaced.remove(&id);
        if let Some(serial) = self.nodes.remove(&id).and_then(|node| node.serial) {
            self.slots.entries.remove(&serial);
            self.slots.save();
        }
        self.ports.retain(|_, port| port.node_id != id);
    }

    pub fn node(&self, id: u32) -> Option<&NodeInfo> {
        self.nodes.get(&id)
    }

    pub fn node_key(&self, id: u32) -> Option<&str> {
        self.nodes.get(&id).map(|node| node.key.as_str())
    }

    pub fn add_port(&mut self, id: u32, info: PortInfo) {
        self.ports.insert(id, info);
    }

    pub fn remove_port(&mut self, id: u32) {
        self.ports.remove(&id);
    }

    pub fn port(&self, id: u32) -> Option<&PortInfo> {
        self.ports.get(&id)
    }

    pub fn ports(&self) -> impl Iterator<Item = (u32, &PortInfo)> {
        self.ports.iter().map(|(id, port)| (*id, port))
    }

    pub fn port_is_on_stream(&self, port: u32) -> bool {
        self.port(port)
            .and_then(|info| self.node(info.node_id))
            .is_some_and(|node| node.is_stream)
    }

    pub fn clear(&mut self) {
        self.reservation_displaced.clear();
        self.nodes.clear();
        self.ports.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{node_layout::NodeLayout, node_volume_store::NodeVolumeStore};

    fn add(topology: &mut Topology, id: u32, key: &str) -> String {
        topology.add_node(
            id,
            NodeInfo {
                serial: Some(u64::from(id)),
                node_name: key.to_string(),
                key: key.to_string(),
                is_meter: false,
                is_stream: true,
            },
        )
    }

    #[test]
    fn duplicate_streams_keep_distinct_saved_state() {
        let mut topology = Topology::default();
        let first = add(&mut topology, 1, "Firefox");
        let second = add(&mut topology, 2, "Firefox");
        assert_eq!(first, "Firefox");
        assert_eq!(second, "Firefox#2");
        assert_eq!(topology.node_key(2), Some(second.as_str()));

        let mut layout = NodeLayout::default();
        let mut volumes = NodeVolumeStore::default();
        assert_eq!(layout.position(&first), None);
        assert_eq!(volumes.volume(&second), None);
        layout.set_position(first.clone(), 10.0, 20.0);
        layout.set_position(second.clone(), 30.0, 40.0);
        volumes.set_volume(&first, 0.4);
        volumes.set_volume(&second, 0.8);
        assert_eq!(layout.position(&first), Some((10.0, 20.0)));
        assert_eq!(layout.position(&second), Some((30.0, 40.0)));
        assert_eq!(volumes.volume(&first), Some(0.4));
        assert_eq!(volumes.volume(&second), Some(0.8));
    }

    #[test]
    fn removed_keys_are_reused_lowest_first() {
        let mut topology = Topology::default();
        assert_eq!(add(&mut topology, 1, "Firefox"), "Firefox");
        assert_eq!(add(&mut topology, 2, "Firefox"), "Firefox#2");
        assert_eq!(add(&mut topology, 3, "Firefox"), "Firefox#3");
        topology.remove_node(2);
        assert_eq!(add(&mut topology, 4, "Firefox"), "Firefox#2");
        topology.remove_node(1);
        assert_eq!(add(&mut topology, 5, "Firefox"), "Firefox");
        topology.clear();
        assert_eq!(add(&mut topology, 6, "Firefox"), "Firefox");
    }

    #[test]
    fn literal_suffixes_do_not_collide() {
        let mut topology = Topology::default();
        add(&mut topology, 1, "Firefox#2");
        add(&mut topology, 2, "Firefox");
        assert_eq!(add(&mut topology, 3, "Firefox"), "Firefox#3");
    }

    #[test]
    fn duplicate_streams_keep_original_name_for_filters() {
        let mut topology = Topology::default();
        add(&mut topology, 1, "Firefox");
        add(&mut topology, 2, "Firefox");
        for id in [1, 2] {
            assert_eq!(topology.node(id).unwrap().node_name, "Firefox");
        }
    }

    #[test]
    fn survivor_keeps_slot_across_restart() {
        let mut before = Topology::default();
        add(&mut before, 1, "Firefox");
        add(&mut before, 2, "Firefox");
        before.remove_node(1);
        let mut after = Topology {
            slots: before.slots,
            enumerating: true,
            ..Topology::default()
        };
        assert_eq!(add(&mut after, 2, "Firefox"), "Firefox#2");
        assert!(after.finish_enumeration().is_empty());
        assert_eq!(add(&mut after, 3, "Firefox"), "Firefox");
    }

    #[test]
    fn enumeration_reserves_slots_then_releases_absent_serials() {
        let mut topology = Topology::default();
        add(&mut topology, 1, "Firefox");
        add(&mut topology, 2, "Firefox");
        topology.clear();
        topology.enumerating = true;
        assert_eq!(add(&mut topology, 3, "Firefox"), "Firefox#3");
        assert_eq!(add(&mut topology, 2, "Firefox"), "Firefox#2");
        assert_eq!(
            topology.finish_enumeration(),
            vec![(3, "Firefox#3".into(), "Firefox".into())]
        );
        assert!(!topology.slots.entries.contains_key(&1));
        assert_eq!(add(&mut topology, 4, "Firefox"), "Firefox#3");
    }

    #[test]
    fn restarted_streams_reclaim_stale_reserved_keys_in_serial_order() {
        let mut topology = Topology::default();
        add(&mut topology, 1, "TestApp");
        add(&mut topology, 2, "TestApp");
        topology.clear();
        topology.enumerating = true;
        assert_eq!(add(&mut topology, 4, "TestApp"), "TestApp#3");
        assert_eq!(add(&mut topology, 3, "TestApp"), "TestApp#4");
        assert_eq!(
            topology.finish_enumeration(),
            vec![
                (3, "TestApp#4".into(), "TestApp".into()),
                (4, "TestApp#3".into(), "TestApp#2".into()),
            ]
        );
        assert_eq!(topology.node_key(3), Some("TestApp"));
        assert_eq!(topology.node_key(4), Some("TestApp#2"));
        assert_eq!(topology.slots.entries[&4].1, "TestApp#2");
        assert!(topology.finish_enumeration().is_empty());
    }

    #[test]
    fn remembered_slot_requires_matching_base_and_free_key() {
        let mut topology = Topology::default();
        topology
            .slots
            .entries
            .insert(1, ("Other".into(), "Other#2".into()));
        assert_eq!(add(&mut topology, 1, "Firefox"), "Firefox");
        topology
            .slots
            .entries
            .insert(2, ("Firefox".into(), "Firefox".into()));
        assert_eq!(add(&mut topology, 2, "Firefox"), "Firefox#2");
    }
}
