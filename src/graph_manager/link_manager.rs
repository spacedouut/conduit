// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{HashMap, HashSet};

use pipewire::spa::utils::Direction;

use super::topology::Topology;
use crate::virtual_device_store::{SavedLink, Store};

/// How long to wait after a link disappears before deciding it was removed on
/// purpose. If an endpoint goes away in that window (app closed, Conduit or
/// PipeWire shutting down) the link is kept and restored later.
pub(super) const LINK_FORGET_DELAY: std::time::Duration = std::time::Duration::from_millis(1000);

/// Tracks live links and keeps the links of saved virtual devices in sync
/// with the store.
#[derive(Default)]
pub(super) struct LinkManager {
    /// Links the user asked for in Conduit, by `(output, input)` port.
    user_links: HashSet<(u32, u32)>,
    link_ports: HashMap<u32, (u32, u32)>,
    /// Saved links that just disappeared, with the ports they joined, waiting
    /// on `LINK_FORGET_DELAY`; they are not restored meanwhile. If one of
    /// those ports goes away first, the endpoint left and the link is kept.
    suspended_links: HashMap<SavedLink, (u32, u32)>,
    requested_links: HashSet<(u32, u32)>,
}

impl LinkManager {
    pub fn is_linked(&self, port_from: u32, port_to: u32) -> bool {
        self.link_ports
            .values()
            .any(|ports| *ports == (port_from, port_to))
    }

    /// Remember that the user asked to toggle this link from Conduit.
    pub fn user_toggled(&mut self, port_from: u32, port_to: u32) {
        if !self.is_linked(port_from, port_to) {
            self.user_links.insert((port_from, port_to));
        }
    }

    pub fn link_added(
        &mut self,
        id: u32,
        port_from: u32,
        port_to: u32,
        topology: &Topology,
        store: &mut Store,
    ) {
        self.link_ports.insert(id, (port_from, port_to));
        self.requested_links.remove(&(port_from, port_to));
        let user_made = self.user_links.remove(&(port_from, port_to));
        if let Some(saved) = saved_link(topology, store, port_from, port_to) {
            self.suspended_links.remove(&saved);
            // The session manager links app streams to the default device on
            // its own; only keep those when the user made them here.
            if user_made
                || !(topology.port_is_on_stream(port_from) || topology.port_is_on_stream(port_to))
            {
                store.add_link(saved);
            }
        }
    }

    /// Forget a live link. Returns the saved link it matched, which should be
    /// passed to [`Self::settle_removed_link`] after `LINK_FORGET_DELAY`.
    pub fn link_removed(
        &mut self,
        id: u32,
        topology: &Topology,
        store: &Store,
    ) -> Option<SavedLink> {
        let (from, to) = self.link_ports.remove(&id)?;
        let saved = saved_link(topology, store, from, to)?;
        self.suspended_links.insert(saved.clone(), (from, to));
        Some(saved)
    }

    pub fn port_removed(&mut self, id: u32) {
        self.suspended_links
            .retain(|_, (from, to)| *from != id && *to != id);
        self.requested_links
            .retain(|(from, to)| *from != id && *to != id);
    }

    pub fn settle_removed_link(&mut self, link: SavedLink, topology: &Topology, store: &mut Store) {
        if self.suspended_links.remove(&link).is_none() {
            return;
        }
        let (outputs, inputs) = resolve_link(topology, &link);
        let still_linked = self
            .link_ports
            .values()
            .any(|(from, to)| outputs.contains(from) && inputs.contains(to));
        if !outputs.is_empty() && !inputs.is_empty() && !still_linked {
            log::info!("Forgetting link that was removed on purpose: {link:?}");
            store.remove_link(&link);
        }
    }

    /// Port pairs of saved links that should be reconnected now. They are
    /// marked as requested so they are only asked for once.
    pub fn links_to_restore(&mut self, topology: &Topology, store: &Store) -> Vec<(u32, u32)> {
        let mut restore = Vec::new();
        for link in store
            .links()
            .iter()
            .filter(|link| !self.suspended_links.contains_key(*link))
        {
            let (outputs, inputs) = resolve_link(topology, link);
            for &port_from in &outputs {
                for &port_to in &inputs {
                    if self.is_linked(port_from, port_to)
                        || !self.requested_links.insert((port_from, port_to))
                    {
                        continue;
                    }
                    log::info!("Restoring link {link:?}");
                    restore.push((port_from, port_to));
                }
            }
        }
        restore
    }

    pub fn clear(&mut self) {
        self.link_ports.clear();
        self.requested_links.clear();
        self.user_links.clear();
    }
}

/// The persistent form of a link, if it touches a saved virtual device.
fn saved_link(
    topology: &Topology,
    store: &Store,
    output_port: u32,
    input_port: u32,
) -> Option<SavedLink> {
    let output = topology.port(output_port)?;
    let input = topology.port(input_port)?;
    let link = SavedLink {
        output_node: topology.node_key(output.node_id)?.to_string(),
        output_port: output.name.clone(),
        input_node: topology.node_key(input.node_id)?.to_string(),
        input_port: input.name.clone(),
    };
    (store.contains_node(&link.output_node) || store.contains_node(&link.input_node))
        .then_some(link)
}

/// Port ids currently matching each end of a saved link.
fn resolve_link(topology: &Topology, link: &SavedLink) -> (Vec<u32>, Vec<u32>) {
    let find = |node: &str, port: &str, direction: Direction| {
        topology
            .ports()
            .filter(|(_, info)| {
                info.direction == direction
                    && info.name == port
                    && topology.node_key(info.node_id) == Some(node)
            })
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
    };
    (
        find(&link.output_node, &link.output_port, Direction::Output),
        find(&link.input_node, &link.input_port, Direction::Input),
    )
}
