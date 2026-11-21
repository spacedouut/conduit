// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;

use pipewire::spa::utils::Direction;

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
}

impl Topology {
    pub fn add_node(&mut self, id: u32, mut info: NodeInfo) -> String {
        let base = info.key.clone();
        let mut suffix = 2;
        while self
            .nodes
            .iter()
            .any(|(node_id, node)| *node_id != id && node.key == info.key)
        {
            info.key = format!("{base}#{suffix}");
            suffix += 1;
        }
        let key = info.key.clone();
        self.nodes.insert(id, info);
        key
    }

    pub fn remove_node(&mut self, id: u32) {
        self.nodes.remove(&id);
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
}
