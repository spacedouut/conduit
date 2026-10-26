// Copyright 2021 Tom A. Wagner <tom.a.wagner@protonmail.com>
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License version 3 as published by
// the Free Software Foundation.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.
//
// SPDX-License-Identifier: GPL-3.0-only

use adw::{
    glib,
    gtk::{self, graphene::Point},
    prelude::*,
    subclass::prelude::*,
};

use pipewire::{channel::Sender as PwSender, spa::utils::Direction};

use crate::{
    ui::{
        graph::{GraphView, Node},
        Window,
    },
    virtual_device_store::SavedLink,
    GtkMessage, PipewireMessage, VirtualDeviceChannels, VirtualDeviceKind,
};

/// How long to wait after a link disappears before deciding it was removed on
/// purpose. If an endpoint goes away in that window (app closed, Conduit or
/// PipeWire shutting down) the link is kept and restored later.
const LINK_FORGET_DELAY: std::time::Duration = std::time::Duration::from_millis(1000);

/// Volume sliders use the cubic scale desktop mixers show, so 50% here
/// matches 50% in pavucontrol or GNOME Settings.
fn slider_from_linear(volume: f32) -> f64 {
    (volume.max(0.0) as f64).cbrt()
}

fn linear_from_slider(value: f64) -> f32 {
    value.max(0.0).powi(3) as f32
}

#[derive(Clone)]
struct PortInfo {
    node_id: u32,
    name: String,
    direction: Direction,
}

#[derive(Clone)]
pub(super) struct VirtualDevice {
    name: String,
    kind: VirtualDeviceKind,
    channels: VirtualDeviceChannels,
}

#[derive(Clone, Copy)]
struct PortVisibility {
    node_id: u32,
    is_monitor: bool,
    is_meter: bool,
}

mod imp {
    use super::*;

    use std::{
        cell::Cell,
        cell::OnceCell,
        cell::RefCell,
        collections::{HashMap, HashSet},
    };

    use crate::{ui::graph, MediaType, NodeType};

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::GraphManager)]
    pub struct GraphManager {
        #[property(get, set, construct_only)]
        pub graph: OnceCell<crate::ui::graph::GraphView>,

        #[property(get, set, construct_only)]
        pub connection_banner: OnceCell<adw::Banner>,

        #[property(get, set, construct_only)]
        pub window: OnceCell<crate::ui::Window>,

        pub pw_sender: OnceCell<PwSender<crate::GtkMessage>>,
        pub items: RefCell<HashMap<u32, glib::Object>>,
        node_is_meter: RefCell<HashMap<u32, bool>>,
        pub(super) node_layout_keys: RefCell<HashMap<u32, String>>,
        pub(super) node_layout: RefCell<crate::node_layout::NodeLayout>,
        port_visibility: RefCell<HashMap<u32, super::PortVisibility>>,
        pub(super) show_monitor_ports: Cell<bool>,
        pub(super) show_meters: Cell<bool>,
        pub(super) show_volume_sliders: Cell<bool>,
        pub(super) volume_slider_setting:
            RefCell<crate::volume_slider_setting::VolumeSliderSetting>,
        pub(super) node_filter: RefCell<crate::node_filter::NodeFilter>,
        pub(super) virtual_devices: RefCell<HashMap<u32, super::VirtualDevice>>,
        pub(super) saved_virtual_devices: RefCell<crate::virtual_device_store::Store>,
        /// Whether to save each requested device, and its node name.
        pub(super) pending_virtual_device_saves: RefCell<HashMap<u32, (bool, String)>>,
        pub next_virtual_device_id: Cell<u32>,
        port_info: RefCell<HashMap<u32, super::PortInfo>>,
        node_is_stream: RefCell<HashMap<u32, bool>>,
        /// Links the user asked for in Conduit, by `(output, input)` port.
        user_links: RefCell<HashSet<(u32, u32)>>,
        link_ports: RefCell<HashMap<u32, (u32, u32)>>,
        /// Saved links that just disappeared, with the ports they joined, waiting
        /// on `LINK_FORGET_DELAY`; they are not restored meanwhile. If one of
        /// those ports goes away first, the endpoint left and the link is kept.
        suspended_links: RefCell<HashMap<SavedLink, (u32, u32)>>,
        requested_links: RefCell<HashSet<(u32, u32)>>,
        pending_volume_restores: RefCell<HashSet<u32>>,
        link_restore_queued: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GraphManager {
        const NAME: &'static str = "ConduitGraphManager";
        type Type = super::GraphManager;
        type ParentType = glib::Object;
    }

    #[glib::derived_properties]
    impl ObjectImpl for GraphManager {}

    impl GraphManager {
        pub async fn receive(&self, receiver: async_channel::Receiver<crate::PipewireMessage>) {
            loop {
                let Ok(msg) = receiver.recv().await else {
                    continue;
                };
                match msg {
                    PipewireMessage::NodeAdded {
                        id,
                        name,
                        layout_key,
                        node_type,
                        is_meter,
                        is_stream,
                    } => {
                        self.node_is_stream.borrow_mut().insert(id, is_stream);
                        self.add_node(id, name.as_str(), layout_key, node_type, is_meter)
                    }
                    PipewireMessage::NodeNameChanged {
                        id,
                        name,
                        media_name,
                    } => self.node_name_changed(id, &name, &media_name),
                    PipewireMessage::NodeVolumeChanged { id, volume } => {
                        self.node_volume_changed(id, volume)
                    }
                    PipewireMessage::PortAdded {
                        id,
                        node_id,
                        name,
                        direction,
                        is_monitor,
                        is_meter,
                    } => self.add_port(id, name.as_str(), node_id, direction, is_monitor, is_meter),
                    PipewireMessage::PortFormatChanged { id, media_type } => {
                        self.port_media_type_changed(id, media_type)
                    }
                    PipewireMessage::LinkAdded {
                        id,
                        port_from,
                        port_to,
                        active,
                        media_type,
                    } => self.add_link(id, port_from, port_to, active, media_type),
                    PipewireMessage::LinkStateChanged { id, active } => {
                        self.link_state_changed(id, active)
                    }
                    PipewireMessage::LinkFormatChanged { id, media_type } => {
                        self.link_format_changed(id, media_type)
                    }
                    PipewireMessage::NodeRemoved { id } => self.remove_node(id),
                    PipewireMessage::PortRemoved { id, node_id } => self.remove_port(id, node_id),
                    PipewireMessage::LinkRemoved { id } => self.remove_link(id),
                    PipewireMessage::VirtualDeviceCreated {
                        id,
                        name,
                        kind,
                        channels,
                    } => self.virtual_device_created(id, name, kind, channels),
                    PipewireMessage::VirtualDeviceRemoved { id } => self.virtual_device_removed(id),
                    PipewireMessage::VirtualDeviceError { message } => {
                        self.pending_virtual_device_saves.borrow_mut().clear();
                        self.show_virtual_device_error(&message)
                    }
                    PipewireMessage::Connecting => {
                        self.obj().connection_banner().set_revealed(true);
                    }
                    PipewireMessage::Connected => {
                        self.obj().connection_banner().set_revealed(false);
                        self.recreate_saved_virtual_devices();
                    }
                    PipewireMessage::Disconnected => {
                        self.clear();
                        self.virtual_devices.borrow_mut().clear();
                        self.obj()
                            .window()
                            .manage_virtual_devices_button()
                            .set_sensitive(false);
                    }
                };
            }
        }

        /// Add a new node to the view.
        fn add_node(
            &self,
            id: u32,
            name: &str,
            layout_key: String,
            node_type: Option<NodeType>,
            is_meter: bool,
        ) {
            log::info!("Adding node to graph: id {}", id);

            let node = graph::Node::new(name, id);
            node.set_show_volume_sliders(self.show_volume_sliders.get());
            let sender = self
                .pw_sender
                .get()
                .expect("pw_sender should be set")
                .clone();
            node.volume_scale().connect_value_changed(
                glib::clone!(@strong sender, @weak node => move |scale| {
                    if node.is_syncing_volume() {
                        return;
                    }
                    if (scale.value() - 1.0).abs() <= 0.02 && scale.value() != 1.0 {
                        scale.set_value(1.0);
                        return;
                    }
                    sender.send(GtkMessage::SetNodeVolume {
                        node_id: node.pipewire_id(),
                        volume: super::linear_from_slider(scale.value()),
                    }).expect("Failed to send message");
                }),
            );
            node.connect_local("disconnect-all", false, glib::clone!(@strong sender => @default-return None, move |args| {
                let node = args[0].get::<graph::Node>().unwrap();
                sender.send(GtkMessage::DisconnectNode { node_id: node.pipewire_id() }).expect("Failed to send message");
                None
            }));
            node.connect_local(
                "show-volume-sliders-changed",
                false,
                glib::clone!(@weak self as manager => @default-return None, move |args| {
                    manager.obj().set_show_volume_sliders(args[1].get::<bool>().unwrap());
                    None
                }),
            );

            if self
                .saved_virtual_devices
                .borrow()
                .volume(&layout_key)
                .is_some()
            {
                self.pending_volume_restores.borrow_mut().insert(id);
            }

            self.items.borrow_mut().insert(id, node.clone().upcast());
            self.node_is_meter.borrow_mut().insert(id, is_meter);
            self.node_layout_keys
                .borrow_mut()
                .insert(id, layout_key.clone());

            self.obj().graph().add_node(node.clone(), node_type);
            if let Some((x, y)) = self.node_layout.borrow().position(&layout_key) {
                self.obj()
                    .graph()
                    .set_node_position(&node, &Point::new(x, y));
            }
            self.refresh_item_visibility();
        }

        /// Update a node tooltip to the view.
        fn node_name_changed(&self, id: u32, node_name: &str, media_name: &str) {
            let items = self.items.borrow();

            let Some(node) = items.get(&id) else {
                log::warn!("Node (id: {id}) for changed name not found in graph manager");
                return;
            };
            let Some(node) = node.dynamic_cast_ref::<graph::Node>() else {
                log::warn!("Graph Manager item under node (id: {id}) is not a node");
                return;
            };

            node.set_node_name(node_name);
            node.set_media_name(media_name);
        }

        fn node_volume_changed(&self, id: u32, volume: f32) {
            let Some(item) = self.items.borrow().get(&id).cloned() else {
                return;
            };
            let Ok(node) = item.dynamic_cast::<graph::Node>() else {
                return;
            };
            let Some(key) = self.node_layout_keys.borrow().get(&id).cloned() else {
                return;
            };

            if self.pending_volume_restores.borrow_mut().remove(&id) {
                let saved = self.saved_virtual_devices.borrow().volume(&key);
                if let Some(saved) = saved.filter(|saved| (saved - volume).abs() > 0.0001) {
                    log::info!("Restoring volume {saved} on {key}");
                    self.pw_sender
                        .get()
                        .expect("pw_sender should be set")
                        .send(GtkMessage::SetNodeVolume {
                            node_id: id,
                            volume: saved,
                        })
                        .expect("Failed to send message");
                    node.set_remote_volume(super::slider_from_linear(saved));
                    return;
                }
            }

            node.set_remote_volume(super::slider_from_linear(volume));
            if self.saved_virtual_devices.borrow().contains_node(&key) {
                self.saved_virtual_devices
                    .borrow_mut()
                    .set_volume(&key, volume);
            }
        }

        /// Remove the node with the specified id from the view.
        fn remove_node(&self, id: u32) {
            log::info!("Removing node from graph: id {}", id);

            self.node_is_meter.borrow_mut().remove(&id);
            self.node_layout_keys.borrow_mut().remove(&id);
            self.pending_volume_restores.borrow_mut().remove(&id);
            self.node_is_stream.borrow_mut().remove(&id);
            self.port_visibility
                .borrow_mut()
                .retain(|_, port| port.node_id != id);

            let Some(node) = self.items.borrow_mut().remove(&id) else {
                log::warn!("Unknown node (id={id}) removed from graph");
                return;
            };
            let Ok(node) = node.dynamic_cast::<graph::Node>() else {
                log::warn!("Graph Manager item under node id {id} is not a node");
                return;
            };

            self.obj().graph().remove_node(&node);
        }

        /// Add a new port to the view.
        fn add_port(
            &self,
            id: u32,
            name: &str,
            node_id: u32,
            direction: pipewire::spa::utils::Direction,
            is_monitor: bool,
            is_meter: bool,
        ) {
            log::info!("Adding port to graph: id {}", id);

            let mut items = self.items.borrow_mut();

            let Some(node) = items.get(&node_id) else {
                log::warn!("Node (id: {node_id}) for port (id: {id}) not found in graph manager");
                return;
            };
            let Ok(node) = node.clone().dynamic_cast::<graph::Node>() else {
                log::warn!("Graph Manager item under node id {node_id} is not a node");
                return;
            };

            let port = graph::Port::new(id, name, direction);
            self.port_info.borrow_mut().insert(
                id,
                super::PortInfo {
                    node_id,
                    name: name.to_string(),
                    direction,
                },
            );

            self.port_visibility.borrow_mut().insert(
                id,
                super::PortVisibility {
                    node_id,
                    is_monitor,
                    is_meter,
                },
            );

            // Create or delete a link if the widget emits the "port-toggled" signal.
            port.connect_local(
                "port_toggled",
                false,
                glib::clone!(@weak self as app => @default-return None, move |args| {
                    // Args always look like this: &[widget, id_port_from, id_port_to]
                    let port_from = args[1].get::<u32>().unwrap();
                    let port_to = args[2].get::<u32>().unwrap();

                    app.toggle_link(port_from, port_to);

                    None
                }),
            );

            items.insert(id, port.clone().upcast());

            node.add_port(port);
            drop(items);
            self.refresh_item_visibility();
            self.queue_link_restore();
        }

        fn port_media_type_changed(&self, id: u32, media_type: MediaType) {
            let items = self.items.borrow();

            let Some(port) = items.get(&id) else {
                log::warn!("Port (id: {id}) for changed media type not found in graph manager");
                return;
            };
            let Some(port) = port.dynamic_cast_ref::<graph::Port>() else {
                log::warn!("Graph Manager item under port id {id} is not a port");
                return;
            };

            port.set_media_type(media_type.as_raw())
        }

        /// Remove the port with the id `id` from the node with the id `node_id`
        /// from the view.
        fn remove_port(&self, id: u32, node_id: u32) {
            log::info!("Removing port from graph: id {}, node_id: {}", id, node_id);

            self.port_visibility.borrow_mut().remove(&id);
            self.port_info.borrow_mut().remove(&id);
            self.suspended_links
                .borrow_mut()
                .retain(|_, (from, to)| *from != id && *to != id);
            self.requested_links
                .borrow_mut()
                .retain(|(from, to)| *from != id && *to != id);

            let mut items = self.items.borrow_mut();

            let Some(node) = items.get(&node_id) else {
                log::warn!("Node (id: {node_id}) for port (id: {id}) not found in graph manager");
                return;
            };
            let Ok(node) = node.clone().dynamic_cast::<graph::Node>() else {
                log::warn!("Graph Manager item under node id {node_id} is not a node");
                return;
            };
            let Some(port) = items.remove(&id) else {
                log::warn!("Unknown Port (id: {id}) removed from graph");
                return;
            };
            let Ok(port) = port.dynamic_cast::<graph::Port>() else {
                log::warn!("Graph Manager item under port id {id} is not a port");
                return;
            };

            node.remove_port(&port);
        }

        /// Add a new link to the view.
        fn add_link(
            &self,
            id: u32,
            output_port_id: u32,
            input_port_id: u32,
            active: bool,
            media_type: MediaType,
        ) {
            log::info!("Adding link to graph: id {}", id);

            self.link_ports
                .borrow_mut()
                .insert(id, (output_port_id, input_port_id));
            self.requested_links
                .borrow_mut()
                .remove(&(output_port_id, input_port_id));
            let user_made = self
                .user_links
                .borrow_mut()
                .remove(&(output_port_id, input_port_id));
            if let Some(saved) = self.saved_link(output_port_id, input_port_id) {
                self.suspended_links.borrow_mut().remove(&saved);
                // The session manager links app streams to the default device on
                // its own; only keep those when the user made them here.
                if user_made || !self.link_touches_stream(output_port_id, input_port_id) {
                    self.saved_virtual_devices.borrow_mut().add_link(saved);
                }
            }

            let mut items = self.items.borrow_mut();

            let Some(output_port) = items.get(&output_port_id) else {
                log::warn!("Output port (id: {output_port_id}) for link (id: {id}) not found in graph manager");
                return;
            };
            let Ok(output_port) = output_port.clone().dynamic_cast::<graph::Port>() else {
                log::warn!("Graph Manager item under port id {output_port_id} is not a port");
                return;
            };
            let Some(input_port) = items.get(&input_port_id) else {
                log::warn!("Output port (id: {input_port_id}) for link (id: {id}) not found in graph manager");
                return;
            };
            let Ok(input_port) = input_port.clone().dynamic_cast::<graph::Port>() else {
                log::warn!("Graph Manager item under port id {input_port_id} is not a port");
                return;
            };

            let link = graph::Link::new();
            link.set_output_port(Some(&output_port));
            link.set_input_port(Some(&input_port));
            link.set_active(active);
            link.set_media_type(media_type);

            items.insert(id, link.clone().upcast());

            // Update graph to contain the new link.
            self.graph
                .get()
                .expect("graph should be set")
                .add_link(link);
            drop(items);
            self.refresh_item_visibility();
        }

        fn link_state_changed(&self, id: u32, active: bool) {
            log::info!(
                "Link state changed: Link (id={id}) is now {}",
                if active { "active" } else { "inactive" }
            );

            let items = self.items.borrow();

            let Some(link) = items.get(&id) else {
                log::warn!("Link state changed on unknown link (id={id})");
                return;
            };
            let Some(link) = link.dynamic_cast_ref::<graph::Link>() else {
                log::warn!("Graph Manager item under link id {id} is not a link");
                return;
            };

            link.set_active(active);
        }

        fn link_format_changed(
            &self,
            id: u32,
            media_type: pipewire::spa::param::format::MediaType,
        ) {
            let items = self.items.borrow();

            let Some(link) = items.get(&id) else {
                log::warn!("Link (id: {id}) for changed media type not found in graph manager");
                return;
            };
            let Some(link) = link.dynamic_cast_ref::<graph::Link>() else {
                log::warn!("Graph Manager item under link id {id} is not a link");
                return;
            };
            link.set_media_type(media_type);
        }

        // Toggle a link between the two specified ports on the remote pipewire server.
        fn toggle_link(&self, port_from: u32, port_to: u32) {
            let linked = self
                .link_ports
                .borrow()
                .values()
                .any(|ports| *ports == (port_from, port_to));
            if !linked {
                self.user_links.borrow_mut().insert((port_from, port_to));
            }
            let sender = self.pw_sender.get().expect("pw_sender shoud be set");
            sender
                .send(crate::GtkMessage::ToggleLink { port_from, port_to })
                .expect("Failed to send message");
        }

        /// Remove the link with the specified id from the view.
        fn remove_link(&self, id: u32) {
            log::info!("Removing link from graph: id {}", id);

            let ports = self.link_ports.borrow_mut().remove(&id);
            if let Some((ports, saved)) =
                ports.and_then(|(from, to)| Some(((from, to), self.saved_link(from, to)?)))
            {
                self.suspended_links
                    .borrow_mut()
                    .insert(saved.clone(), ports);
                let manager = self.obj().downgrade();
                glib::timeout_add_local_once(super::LINK_FORGET_DELAY, move || {
                    if let Some(manager) = manager.upgrade() {
                        manager.imp().settle_removed_link(saved);
                    }
                });
            }

            let Some(link) = self.items.borrow_mut().remove(&id) else {
                log::warn!("Unknown Link (id={id}) removed from graph");
                return;
            };
            let Ok(link) = link.dynamic_cast::<graph::Link>() else {
                log::warn!("Graph Manager item under link id {id} is not a link");
                return;
            };

            self.obj().graph().remove_link(&link);
        }

        fn clear(&self) {
            self.items.borrow_mut().clear();
            self.node_is_meter.borrow_mut().clear();
            self.node_layout_keys.borrow_mut().clear();
            self.port_visibility.borrow_mut().clear();
            self.port_info.borrow_mut().clear();
            self.link_ports.borrow_mut().clear();
            self.requested_links.borrow_mut().clear();
            self.pending_volume_restores.borrow_mut().clear();
            self.node_is_stream.borrow_mut().clear();
            self.user_links.borrow_mut().clear();
            self.obj().graph().clear();
        }

        /// The persistent form of a link, if it touches a saved virtual device.
        fn saved_link(&self, output_port: u32, input_port: u32) -> Option<SavedLink> {
            let port_info = self.port_info.borrow();
            let keys = self.node_layout_keys.borrow();
            let output = port_info.get(&output_port)?;
            let input = port_info.get(&input_port)?;
            let link = SavedLink {
                output_node: keys.get(&output.node_id)?.clone(),
                output_port: output.name.clone(),
                input_node: keys.get(&input.node_id)?.clone(),
                input_port: input.name.clone(),
            };
            let store = self.saved_virtual_devices.borrow();
            (store.contains_node(&link.output_node) || store.contains_node(&link.input_node))
                .then_some(link)
        }

        fn link_touches_stream(&self, output_port: u32, input_port: u32) -> bool {
            let port_info = self.port_info.borrow();
            let node_is_stream = self.node_is_stream.borrow();
            [output_port, input_port].iter().any(|port| {
                port_info
                    .get(port)
                    .and_then(|info| node_is_stream.get(&info.node_id))
                    .copied()
                    .unwrap_or(false)
            })
        }

        /// Port ids currently matching each end of a saved link.
        fn resolve_link(&self, link: &SavedLink) -> (Vec<u32>, Vec<u32>) {
            let port_info = self.port_info.borrow();
            let keys = self.node_layout_keys.borrow();
            let find = |node: &str, port: &str, direction: Direction| {
                port_info
                    .iter()
                    .filter(|(_, info)| {
                        info.direction == direction
                            && info.name == port
                            && keys.get(&info.node_id).is_some_and(|key| key == node)
                    })
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>()
            };
            (
                find(&link.output_node, &link.output_port, Direction::Output),
                find(&link.input_node, &link.input_port, Direction::Input),
            )
        }

        fn settle_removed_link(&self, link: SavedLink) {
            if self.suspended_links.borrow_mut().remove(&link).is_none() {
                return;
            }
            let (outputs, inputs) = self.resolve_link(&link);
            let link_ports = self.link_ports.borrow();
            let still_linked = link_ports
                .values()
                .any(|(from, to)| outputs.contains(from) && inputs.contains(to));
            if !outputs.is_empty() && !inputs.is_empty() && !still_linked {
                drop(link_ports);
                log::info!("Forgetting link that was removed on purpose: {link:?}");
                self.saved_virtual_devices.borrow_mut().remove_link(&link);
            }
        }

        fn queue_link_restore(&self) {
            if self.link_restore_queued.replace(true) {
                return;
            }
            let manager = self.obj().downgrade();
            glib::idle_add_local_once(move || {
                if let Some(manager) = manager.upgrade() {
                    manager.imp().link_restore_queued.set(false);
                    manager.imp().restore_saved_links();
                }
            });
        }

        /// Reconnect saved virtual device links whose endpoints both exist.
        fn restore_saved_links(&self) {
            let links = self.saved_virtual_devices.borrow().links().to_vec();
            let suspended = self.suspended_links.borrow().clone();
            for link in links.iter().filter(|link| !suspended.contains_key(link)) {
                let (outputs, inputs) = self.resolve_link(link);
                for &port_from in &outputs {
                    for &port_to in &inputs {
                        let linked = self
                            .link_ports
                            .borrow()
                            .values()
                            .any(|ports| *ports == (port_from, port_to));
                        if linked
                            || !self
                                .requested_links
                                .borrow_mut()
                                .insert((port_from, port_to))
                        {
                            continue;
                        }
                        log::info!("Restoring link {link:?}");
                        self.pw_sender
                            .get()
                            .expect("pw_sender should be set")
                            .send(GtkMessage::CreateLink { port_from, port_to })
                            .expect("Failed to send message");
                    }
                }
            }
        }

        pub(super) fn refresh_item_visibility(&self) {
            let node_is_meter = self.node_is_meter.borrow();
            let port_visibility = self.port_visibility.borrow();
            let items = self.items.borrow();

            for (id, item) in items.iter() {
                if let Some(node) = item.dynamic_cast_ref::<graph::Node>() {
                    let is_meter = node_is_meter.get(id).copied().unwrap_or(false);
                    let key = self
                        .node_layout_keys
                        .borrow()
                        .get(id)
                        .cloned()
                        .unwrap_or_default();
                    let hidden = self
                        .node_filter
                        .borrow()
                        .patterns()
                        .iter()
                        .filter_map(|pattern| regex::Regex::new(pattern).ok())
                        .any(|pattern| pattern.is_match(&key));
                    node.set_visible((self.show_meters.get() || !is_meter) && !hidden);
                }
            }

            for (id, item) in items.iter() {
                if let Some(port) = item.dynamic_cast_ref::<graph::Port>() {
                    let Some(metadata) = port_visibility.get(id) else {
                        continue;
                    };
                    let node_is_visible = items
                        .get(&metadata.node_id)
                        .and_then(|item| item.dynamic_cast_ref::<graph::Node>())
                        .is_some_and(|node| node.is_visible());
                    let is_visible = node_is_visible
                        && (self.show_monitor_ports.get() || !metadata.is_monitor)
                        && (self.show_meters.get() || !metadata.is_meter);
                    port.set_visible(is_visible);
                }
            }

            for item in items.values() {
                if let Some(link) = item.dynamic_cast_ref::<graph::Link>() {
                    let is_visible = link.output_port().is_some_and(|port| port.is_visible())
                        && link.input_port().is_some_and(|port| port.is_visible());
                    link.set_visible(is_visible);
                }
            }

            self.obj().graph().queue_draw();
        }

        fn virtual_device_created(
            &self,
            id: u32,
            name: String,
            kind: VirtualDeviceKind,
            channels: VirtualDeviceChannels,
        ) {
            let save = self
                .pending_virtual_device_saves
                .borrow_mut()
                .remove(&id)
                .is_some_and(|(save, _)| save);
            let device = super::VirtualDevice {
                name: name.clone(),
                kind,
                channels,
            };
            if save {
                self.saved_virtual_devices.borrow_mut().add(
                    crate::virtual_device_store::Definition {
                        name,
                        kind,
                        channels,
                    },
                );
            }
            self.virtual_devices.borrow_mut().insert(id, device);
            self.obj()
                .window()
                .manage_virtual_devices_button()
                .set_sensitive(true);
        }

        fn virtual_device_removed(&self, id: u32) {
            if let Some(device) = self.virtual_devices.borrow_mut().remove(&id) {
                self.saved_virtual_devices.borrow_mut().remove(
                    &crate::virtual_device_store::Definition {
                        name: device.name,
                        kind: device.kind,
                        channels: device.channels,
                    },
                );
            }
            self.obj()
                .window()
                .manage_virtual_devices_button()
                .set_sensitive(!self.virtual_devices.borrow().is_empty());
        }

        pub(super) fn show_virtual_device_error(&self, message: &str) {
            let dialog = adw::MessageDialog::builder()
                .transient_for(&self.obj().window())
                .modal(true)
                .heading("Virtual device operation failed")
                .body(message)
                .close_response("close")
                .build();
            dialog.add_response("close", "Close");
            dialog.present();
        }

        fn recreate_saved_virtual_devices(&self) {
            let devices = self.saved_virtual_devices.borrow().devices();
            for device in devices {
                self.obj().request_create_virtual_device(
                    &device.name,
                    device.kind,
                    device.channels,
                    false,
                );
            }
        }
    }
}

glib::wrapper! {
    pub struct GraphManager(ObjectSubclass<imp::GraphManager>);
}

async fn receive(graph_manager: GraphManager, receiver: async_channel::Receiver<PipewireMessage>) {
    graph_manager.imp().receive(receiver).await
}

impl GraphManager {
    fn set_show_monitor_ports(&self, show: bool) {
        self.imp().show_monitor_ports.set(show);
        self.imp().refresh_item_visibility();
    }

    fn set_show_volume_sliders(&self, show: bool) {
        self.imp().show_volume_sliders.set(show);
        self.imp().volume_slider_setting.borrow_mut().set_show(show);
        for item in self.imp().items.borrow().values() {
            if let Some(node) = item.dynamic_cast_ref::<crate::ui::graph::Node>() {
                node.set_show_volume_sliders(show);
            }
        }
        self.graph().queue_allocate();
    }

    fn node_position_changed(&self, node: &Node, position: Point) {
        let id = node.pipewire_id();
        let Some(layout_key) = self.imp().node_layout_keys.borrow().get(&id).cloned() else {
            return;
        };
        self.imp()
            .node_layout
            .borrow_mut()
            .set_position(layout_key, position.x(), position.y());
    }

    fn setup_virtual_device_controls(&self) {
        let window = self.window();
        window.create_virtual_device_button().connect_clicked(
            glib::clone!(@weak self as manager => move |_| {
                manager.show_create_virtual_device_dialog();
            }),
        );
        window.manage_virtual_devices_button().connect_clicked(
            glib::clone!(@weak self as manager => move |_| {
                manager.show_manage_virtual_devices_dialog();
            }),
        );
        window.show_monitor_ports_button().connect_toggled(
            glib::clone!(@weak self as manager => move |button| {
                manager.set_show_monitor_ports(button.is_active());
            }),
        );
        window.edit_node_filters_button().connect_clicked(
            glib::clone!(@weak self as manager => move |_| manager.show_node_filter_dialog()),
        );
    }

    fn show_create_virtual_device_dialog(&self) {
        let dialog = gtk::Dialog::with_buttons(
            Some("Create virtual device"),
            Some(&self.window()),
            gtk::DialogFlags::MODAL,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Create", gtk::ResponseType::Accept),
            ],
        );
        dialog.set_default_response(gtk::ResponseType::Accept);
        dialog.set_response_sensitive(gtk::ResponseType::Accept, false);

        let form = gtk::Grid::builder()
            .row_spacing(12)
            .column_spacing(18)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();

        let introduction = gtk::Label::new(Some(
            "Create a routable endpoint. Conduit recreates it, with its volume and connections, every time it starts.",
        ));
        introduction.set_xalign(0.0);
        introduction.set_wrap(true);
        introduction.add_css_class("virtual-device-introduction");
        form.attach(&introduction, 0, 0, 2, 1);

        let name_label = gtk::Label::new(Some("Name"));
        name_label.set_xalign(1.0);
        let name = gtk::Entry::builder()
            .placeholder_text("Podcast mix")
            .activates_default(true)
            .hexpand(true)
            .build();
        form.attach(&name_label, 0, 1, 1, 1);
        form.attach(&name, 1, 1, 1, 1);

        let kind_label = gtk::Label::new(Some("Device type"));
        kind_label.set_xalign(1.0);
        let kind = gtk::ComboBoxText::new();
        kind.append(Some("sink"), "Virtual output");
        kind.append(Some("source"), "Virtual input");
        kind.set_active_id(Some("sink"));
        form.attach(&kind_label, 0, 2, 1, 1);
        form.attach(&kind, 1, 2, 1, 1);

        let channels_label = gtk::Label::new(Some("Channels"));
        channels_label.set_xalign(1.0);
        let channels = gtk::ComboBoxText::new();
        channels.append(Some("stereo"), "Stereo");
        channels.append(Some("mono"), "Mono");
        channels.set_active_id(Some("stereo"));
        form.attach(&channels_label, 0, 3, 1, 1);
        form.attach(&channels, 1, 3, 1, 1);

        dialog.content_area().append(&form);
        name.connect_changed(glib::clone!(@weak dialog => move |entry| {
            dialog.set_response_sensitive(
                gtk::ResponseType::Accept,
                !entry.text().trim().is_empty(),
            );
        }));
        dialog.connect_response(glib::clone!(
            @weak self as manager,
            @weak name,
            @weak kind,
            @weak channels
            => move |dialog, response| {
                if response == gtk::ResponseType::Accept {
                    let device_kind = if kind.active_id().as_deref() == Some("source") {
                        VirtualDeviceKind::Source
                    } else {
                        VirtualDeviceKind::Sink
                    };
                    let device_channels = if channels.active_id().as_deref() == Some("mono") {
                        VirtualDeviceChannels::Mono
                    } else {
                        VirtualDeviceChannels::Stereo
                    };
                    manager.request_create_virtual_device(
                        name.text().trim(),
                        device_kind,
                        device_channels,
                        true,
                    );
                }
                dialog.close();
            }
        ));

        dialog.present();
        name.grab_focus();
    }

    fn show_manage_virtual_devices_dialog(&self) {
        let dialog = gtk::Dialog::with_buttons(
            Some("Virtual devices"),
            Some(&self.window()),
            gtk::DialogFlags::MODAL,
            &[("Done", gtk::ResponseType::Close)],
        );
        let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        list.set_margin_top(16);
        list.set_margin_bottom(16);
        list.set_margin_start(16);
        list.set_margin_end(16);

        let mut devices: Vec<_> = self
            .imp()
            .virtual_devices
            .borrow()
            .iter()
            .map(|(id, device)| (*id, device.clone()))
            .collect();
        devices.sort_by(|left, right| left.1.name.cmp(&right.1.name));

        for (id, device) in devices {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("virtual-device-row");
            let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
            labels.set_hexpand(true);

            let name = gtk::Label::new(Some(&device.name));
            name.set_xalign(0.0);
            let kind = gtk::Label::new(Some(match device.kind {
                VirtualDeviceKind::Sink => "Virtual output",
                VirtualDeviceKind::Source => "Virtual input",
            }));
            kind.set_xalign(0.0);
            kind.add_css_class("dim-label");
            labels.append(&name);
            labels.append(&kind);
            row.append(&labels);

            let remove = gtk::Button::with_label("Remove");
            remove.add_css_class("destructive-action");
            remove.connect_clicked(glib::clone!(
                @weak self as manager,
                @weak dialog
                => move |_| {
                    manager.request_remove_virtual_device(id);
                    dialog.close();
                }
            ));
            row.append(&remove);
            list.append(&row);
        }

        dialog.content_area().append(&list);
        dialog.connect_response(|dialog, _| dialog.close());
        dialog.present();
    }

    fn request_create_virtual_device(
        &self,
        name: &str,
        kind: VirtualDeviceKind,
        channels: VirtualDeviceChannels,
        save: bool,
    ) {
        let imp = self.imp();
        let definition = crate::virtual_device_store::Definition {
            name: name.to_string(),
            kind,
            channels,
        };
        let node_name = definition.node_name();
        // A saved device with this exact definition and no live node is a
        // retry; anything else with the same node name would share its state.
        let taken = imp
            .saved_virtual_devices
            .borrow()
            .devices()
            .iter()
            .any(|saved| saved.node_name() == node_name && *saved != definition)
            || imp
                .pending_virtual_device_saves
                .borrow()
                .values()
                .any(|(_, pending)| *pending == node_name)
            || imp.virtual_devices.borrow().values().any(|device| {
                crate::virtual_device_store::Definition {
                    name: device.name.clone(),
                    kind: device.kind,
                    channels: device.channels,
                }
                .node_name()
                    == node_name
            });
        if save && taken {
            imp.show_virtual_device_error(&format!(
                "A virtual {} with a name like \"{name}\" already exists.",
                match kind {
                    VirtualDeviceKind::Sink => "sink",
                    VirtualDeviceKind::Source => "source",
                }
            ));
            return;
        }
        let id = imp.next_virtual_device_id.get();
        imp.next_virtual_device_id.set(id.wrapping_add(1));
        imp.pending_virtual_device_saves
            .borrow_mut()
            .insert(id, (save, node_name.clone()));
        imp.pw_sender
            .get()
            .expect("pw_sender should be set")
            .send(GtkMessage::CreateVirtualDevice {
                id,
                name: name.to_string(),
                node_name,
                kind,
                channels,
            })
            .expect("Failed to send message");
    }

    fn request_remove_virtual_device(&self, id: u32) {
        self.imp()
            .pw_sender
            .get()
            .expect("pw_sender should be set")
            .send(GtkMessage::RemoveVirtualDevice { id })
            .expect("Failed to send message");
    }

    fn show_node_filter_dialog(&self) {
        let dialog = gtk::Dialog::with_buttons(
            Some("Hidden node patterns"),
            Some(&self.window()),
            gtk::DialogFlags::MODAL,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Save", gtk::ResponseType::Accept),
            ],
        );
        let entry = gtk::TextView::new();
        entry
            .buffer()
            .set_text(&self.imp().node_filter.borrow().patterns().join("\n"));
        entry.set_tooltip_text(Some(
            "One regular expression per line; matched PipeWire node.name values are hidden.",
        ));
        dialog.content_area().append(&entry);
        dialog.connect_response(glib::clone!(@weak self as manager, @weak entry => move |dialog, response| {
            if response == gtk::ResponseType::Accept { let buffer = entry.buffer(); let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false); manager.imp().node_filter.borrow_mut().set_patterns(text.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_owned).collect()); manager.imp().refresh_item_visibility(); }
            dialog.close();
        }));
        dialog.present();
    }

    pub fn new(
        window: &Window,
        graph: &GraphView,
        connection_banner: &adw::Banner,
        sender: PwSender<GtkMessage>,
        receiver: async_channel::Receiver<PipewireMessage>,
    ) -> Self {
        let res: Self = glib::Object::builder()
            .property("graph", graph)
            .property("connection-banner", connection_banner)
            .property("window", window)
            .build();

        res.imp()
            .node_layout
            .replace(crate::node_layout::NodeLayout::load());
        res.imp()
            .node_filter
            .replace(crate::node_filter::NodeFilter::load());
        let show_volume_sliders = res.imp().volume_slider_setting.borrow().show();
        res.imp()
            .saved_virtual_devices
            .replace(crate::virtual_device_store::Store::load());

        glib::MainContext::default().spawn_local(receive(res.clone(), receiver));
        assert!(
            res.imp().pw_sender.set(sender).is_ok(),
            "Should be able to set pw_sender)"
        );

        res.setup_virtual_device_controls();
        graph.set_node_position_changed_handler(glib::clone!(@weak res => move |node, position| {
            res.node_position_changed(node, position);
        }));
        res.set_show_monitor_ports(window.show_monitor_ports_button().is_active());
        res.imp().show_meters.set(true);
        res.imp().show_volume_sliders.set(show_volume_sliders);

        res
    }
}
