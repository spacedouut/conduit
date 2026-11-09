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
    /// Stable `node.name`, used to remember layout, volume and links.
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
    pub fn add_node(&mut self, id: u32, info: NodeInfo) {
        self.nodes.insert(id, info);
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
