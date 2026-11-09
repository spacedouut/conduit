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

//! Bridges PipeWire events from the PipeWire thread to the graph on screen.
//!
//! [`GraphManager`] only coordinates: link bookkeeping lives in
//! [`link_manager`], saved volumes in [`volume_manager`] and virtual devices
//! in [`device_manager`].

mod device_manager;
mod dialogs;
mod link_manager;
mod topology;
mod volume_manager;

use adw::{glib, gtk::graphene::Point, prelude::*, subclass::prelude::*};

use pipewire::channel::Sender as PwSender;

use crate::{
    ui::{
        graph::{GraphView, Node},
        Window,
    },
    GtkMessage, PipewireMessage, VirtualDeviceChannels, VirtualDeviceKind,
};

mod imp {
    use super::*;

    use std::{
        cell::{Cell, OnceCell, RefCell},
        collections::HashMap,
    };

    use super::{
        device_manager::{DeviceManager, VirtualDevice},
        link_manager::{LinkManager, LINK_FORGET_DELAY},
        topology::{NodeInfo, PortInfo, Topology},
        volume_manager::{self, VolumeManager},
    };
    use crate::{ui::graph, MediaType, NodeType};

    const VOLUME_SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

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
        /// Widgets for every PipeWire node, port and link, by id.
        pub items: RefCell<HashMap<u32, glib::Object>>,
        pub(super) topology: RefCell<Topology>,
        pub(super) links: RefCell<LinkManager>,
        pub(super) volumes: RefCell<VolumeManager>,
        pub(super) devices: RefCell<DeviceManager>,
        pub(super) node_layout: RefCell<crate::node_layout::NodeLayout>,
        pub(super) comments: RefCell<crate::comment_store::CommentStore>,
        pub(super) comment_ids: RefCell<HashMap<Node, u32>>,
        pub(super) show_monitor_ports: Cell<bool>,
        pub(super) show_meters: Cell<bool>,
        pub(super) show_volume_sliders: Cell<bool>,
        pub(super) volume_slider_setting:
            RefCell<crate::volume_slider_setting::VolumeSliderSetting>,
        pub(super) node_filter: RefCell<crate::node_filter::NodeFilter>,
        link_restore_queued: Cell<bool>,
        volume_save_queued: Cell<bool>,
    }

    impl Drop for GraphManager {
        fn drop(&mut self) {
            self.volumes.borrow_mut().save();
        }
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
            while let Ok(msg) = receiver.recv().await {
                self.handle_message(msg);
            }
            log::warn!("PipeWire thread closed its channel; no more graph updates");
        }

        fn handle_message(&self, msg: PipewireMessage) {
            match msg {
                PipewireMessage::NodeAdded {
                    id,
                    name,
                    layout_key,
                    node_type,
                    is_meter,
                    is_stream,
                } => self.add_node(
                    id,
                    &name,
                    NodeInfo {
                        key: layout_key,
                        is_meter,
                        is_stream,
                    },
                    node_type,
                ),
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
                } => self.add_port(
                    id,
                    PortInfo {
                        node_id,
                        name,
                        direction,
                        is_monitor,
                        is_meter,
                    },
                ),
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
                } => {
                    self.devices.borrow_mut().created(
                        id,
                        VirtualDevice {
                            name,
                            kind,
                            channels,
                        },
                    );
                    self.update_manage_devices_button();
                }
                PipewireMessage::VirtualDeviceRemoved { id } => {
                    self.devices.borrow_mut().removed(id);
                    self.update_manage_devices_button();
                }
                PipewireMessage::VirtualDeviceError { message } => {
                    self.devices.borrow_mut().request_failed();
                    self.obj().show_virtual_device_error(&message)
                }
                PipewireMessage::Connecting => {
                    self.obj().connection_banner().set_revealed(true);
                }
                PipewireMessage::Connected => {
                    self.obj().connection_banner().set_revealed(false);
                    self.devices.borrow_mut().connected();
                    self.recreate_saved_virtual_devices();
                }
                PipewireMessage::Disconnected => {
                    self.devices.borrow_mut().disconnected();
                    self.clear();
                    self.update_manage_devices_button();
                }
            }
        }

        /// Send a request to the PipeWire thread. If it is gone, log and carry on.
        pub(super) fn send(&self, message: GtkMessage) {
            let Some(sender) = self.pw_sender.get() else {
                log::error!("No PipeWire sender yet; dropping {message:?}");
                return;
            };
            if let Err(message) = sender.send(message) {
                log::error!("PipeWire thread is gone; dropping {message:?}");
            }
        }

        fn item<T: IsA<glib::Object>>(&self, id: u32) -> Option<T> {
            self.items.borrow().get(&id)?.clone().downcast::<T>().ok()
        }

        /// Add a new node to the view.
        fn add_node(&self, id: u32, name: &str, info: NodeInfo, node_type: Option<NodeType>) {
            log::info!("Adding node to graph: id {}", id);

            let node = graph::Node::new(name, id);
            node.set_show_volume_sliders(self.show_volume_sliders.get());
            node.volume_scale().connect_value_changed(
                glib::clone!(@weak self as manager, @weak node => move |scale| {
                    if node.is_syncing_volume() {
                        return;
                    }
                    if (scale.value() - 1.0).abs() <= 0.02 && scale.value() != 1.0 {
                        scale.set_value(1.0);
                        return;
                    }
                    let volume = volume_manager::linear_from_slider(scale.value());
                    manager.user_set_volume(node.pipewire_id(), volume);
                    manager.send(GtkMessage::SetNodeVolume {
                        node_id: node.pipewire_id(),
                        volume,
                    });
                }),
            );
            node.connect_local(
                "disconnect-all",
                false,
                glib::clone!(@weak self as manager => @default-return None, move |args| {
                    if let Some(node) = args.first().and_then(|arg| arg.get::<graph::Node>().ok()) {
                        manager.send(GtkMessage::DisconnectNode { node_id: node.pipewire_id() });
                    }
                    None
                }),
            );
            node.connect_local(
                "show-volume-sliders-changed",
                false,
                glib::clone!(@weak self as manager => @default-return None, move |args| {
                    if let Some(show) = args.get(1).and_then(|arg| arg.get::<bool>().ok()) {
                        manager.obj().set_show_volume_sliders(show);
                    }
                    None
                }),
            );

            self.volumes
                .borrow_mut()
                .node_added(id, &info.key, &self.devices.borrow().store);
            let position = self.node_layout.borrow().position(&info.key);
            self.topology.borrow_mut().add_node(id, info);
            self.items.borrow_mut().insert(id, node.clone().upcast());

            self.obj().graph().add_node(node.clone(), node_type);
            if let Some((x, y)) = position {
                self.obj()
                    .graph()
                    .set_node_position(&node, &Point::new(x, y));
            }
            self.refresh_item_visibility();
        }

        /// Update a node tooltip to the view.
        fn node_name_changed(&self, id: u32, node_name: &str, media_name: &str) {
            let Some(node) = self.item::<graph::Node>(id) else {
                log::warn!("Node (id: {id}) for changed name not found in graph manager");
                return;
            };
            node.set_node_name(node_name);
            node.set_media_name(media_name);
        }

        fn node_volume_changed(&self, id: u32, volume: f32) {
            let Some(node) = self.item::<graph::Node>(id) else {
                return;
            };
            let Some(key) = self.topology.borrow().node_key(id).map(str::to_owned) else {
                return;
            };

            let restore = self.volumes.borrow_mut().remote_volume_changed(
                id,
                &key,
                volume,
                &mut self.devices.borrow_mut().store,
            );
            if let Some(saved) = restore {
                self.send(GtkMessage::SetNodeVolume {
                    node_id: id,
                    volume: saved,
                });
            }
            node.set_remote_volume(volume_manager::slider_from_linear(
                restore.unwrap_or(volume),
            ));
            self.queue_volume_save();
        }

        fn user_set_volume(&self, id: u32, volume: f32) {
            let Some(key) = self.topology.borrow().node_key(id).map(str::to_owned) else {
                return;
            };
            self.volumes.borrow_mut().user_set_volume(
                id,
                &key,
                volume,
                &mut self.devices.borrow_mut().store,
            );
            self.queue_volume_save();
        }

        /// Write remembered volumes shortly, so dragging a slider doesn't
        /// rewrite the file for every step.
        fn queue_volume_save(&self) {
            if !self.volumes.borrow().needs_save() || self.volume_save_queued.replace(true) {
                return;
            }
            let manager = self.obj().downgrade();
            glib::timeout_add_local_once(VOLUME_SAVE_DELAY, move || {
                if let Some(manager) = manager.upgrade() {
                    manager.imp().volume_save_queued.set(false);
                    manager.imp().volumes.borrow_mut().save();
                }
            });
        }

        /// Remove the node with the specified id from the view.
        fn remove_node(&self, id: u32) {
            log::info!("Removing node from graph: id {}", id);

            self.topology.borrow_mut().remove_node(id);
            self.volumes.borrow_mut().node_removed(id);

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
        fn add_port(&self, id: u32, info: PortInfo) {
            log::info!("Adding port to graph: id {}", id);

            let node_id = info.node_id;
            let Some(node) = self.item::<graph::Node>(node_id) else {
                log::warn!("Node (id: {node_id}) for port (id: {id}) not found in graph manager");
                return;
            };

            let port = graph::Port::new(id, &info.name, info.direction);
            self.topology.borrow_mut().add_port(id, info);

            // Create or delete a link if the widget emits the "port-toggled" signal.
            port.connect_local(
                "port_toggled",
                false,
                glib::clone!(@weak self as manager => @default-return None, move |args| {
                    // Args always look like this: &[widget, id_port_from, id_port_to]
                    let port = |index: usize| args.get(index).and_then(|arg| arg.get::<u32>().ok());
                    if let (Some(port_from), Some(port_to)) = (port(1), port(2)) {
                        manager.toggle_link(port_from, port_to);
                    }
                    None
                }),
            );

            self.items.borrow_mut().insert(id, port.clone().upcast());
            node.add_port(port);
            self.refresh_item_visibility();
            self.queue_link_restore();
        }

        fn port_media_type_changed(&self, id: u32, media_type: MediaType) {
            let Some(port) = self.item::<graph::Port>(id) else {
                log::warn!("Port (id: {id}) for changed media type not found in graph manager");
                return;
            };
            port.set_media_type(media_type.as_raw())
        }

        /// Remove the port with the id `id` from the node with the id `node_id`
        /// from the view.
        fn remove_port(&self, id: u32, node_id: u32) {
            log::info!("Removing port from graph: id {}, node_id: {}", id, node_id);

            self.topology.borrow_mut().remove_port(id);
            self.links.borrow_mut().port_removed(id);

            let Some(node) = self.item::<graph::Node>(node_id) else {
                log::warn!("Node (id: {node_id}) for port (id: {id}) not found in graph manager");
                return;
            };
            let Some(port) = self.items.borrow_mut().remove(&id) else {
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

            self.links.borrow_mut().link_added(
                id,
                output_port_id,
                input_port_id,
                &self.topology.borrow(),
                &mut self.devices.borrow_mut().store,
            );

            let Some(output_port) = self.item::<graph::Port>(output_port_id) else {
                log::warn!("Output port (id: {output_port_id}) for link (id: {id}) not found in graph manager");
                return;
            };
            let Some(input_port) = self.item::<graph::Port>(input_port_id) else {
                log::warn!("Input port (id: {input_port_id}) for link (id: {id}) not found in graph manager");
                return;
            };

            let link = graph::Link::new();
            link.set_output_port(Some(&output_port));
            link.set_input_port(Some(&input_port));
            link.set_active(active);
            link.set_media_type(media_type);

            self.items.borrow_mut().insert(id, link.clone().upcast());
            self.obj().graph().add_link(link);
            self.refresh_item_visibility();
        }

        fn link_state_changed(&self, id: u32, active: bool) {
            log::info!(
                "Link state changed: Link (id={id}) is now {}",
                if active { "active" } else { "inactive" }
            );
            let Some(link) = self.item::<graph::Link>(id) else {
                log::warn!("Link state changed on unknown link (id={id})");
                return;
            };
            link.set_active(active);
        }

        fn link_format_changed(&self, id: u32, media_type: MediaType) {
            let Some(link) = self.item::<graph::Link>(id) else {
                log::warn!("Link (id: {id}) for changed media type not found in graph manager");
                return;
            };
            link.set_media_type(media_type);
        }

        // Toggle a link between the two specified ports on the remote pipewire server.
        fn toggle_link(&self, port_from: u32, port_to: u32) {
            self.links.borrow_mut().user_toggled(port_from, port_to);
            self.send(GtkMessage::ToggleLink { port_from, port_to });
        }

        /// Remove the link with the specified id from the view.
        fn remove_link(&self, id: u32) {
            log::info!("Removing link from graph: id {}", id);

            let saved = self.links.borrow_mut().link_removed(
                id,
                &self.topology.borrow(),
                &self.devices.borrow().store,
            );
            if let Some(saved) = saved {
                let manager = self.obj().downgrade();
                glib::timeout_add_local_once(LINK_FORGET_DELAY, move || {
                    if let Some(manager) = manager.upgrade() {
                        let imp = manager.imp();
                        imp.links.borrow_mut().settle_removed_link(
                            saved,
                            &imp.topology.borrow(),
                            &mut imp.devices.borrow_mut().store,
                        );
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
            self.topology.borrow_mut().clear();
            self.links.borrow_mut().clear();
            self.volumes.borrow_mut().clear();
            self.obj().graph().clear();
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
            let restore = self
                .links
                .borrow_mut()
                .links_to_restore(&self.topology.borrow(), &self.devices.borrow().store);
            for (port_from, port_to) in restore {
                self.send(GtkMessage::CreateLink { port_from, port_to });
            }
        }

        pub(super) fn refresh_item_visibility(&self) {
            let topology = self.topology.borrow();
            let items = self.items.borrow();
            let hidden_patterns: Vec<_> = self
                .node_filter
                .borrow()
                .patterns()
                .iter()
                .filter_map(|pattern| regex::Regex::new(pattern).ok())
                .collect();

            for (id, item) in items.iter() {
                if let Some(node) = item.dynamic_cast_ref::<graph::Node>() {
                    let Some(info) = topology.node(*id) else {
                        continue;
                    };
                    let hidden = hidden_patterns
                        .iter()
                        .any(|pattern| pattern.is_match(&info.key));
                    node.set_visible((self.show_meters.get() || !info.is_meter) && !hidden);
                }
            }

            for (id, item) in items.iter() {
                if let Some(port) = item.dynamic_cast_ref::<graph::Port>() {
                    let Some(info) = topology.port(*id) else {
                        continue;
                    };
                    let node_is_visible = items
                        .get(&info.node_id)
                        .and_then(|item| item.dynamic_cast_ref::<graph::Node>())
                        .is_some_and(|node| node.is_visible());
                    let is_visible = node_is_visible
                        && (self.show_monitor_ports.get() || !info.is_monitor)
                        && (self.show_meters.get() || !info.is_meter);
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

        fn update_manage_devices_button(&self) {
            self.obj()
                .window()
                .manage_virtual_devices_button()
                .set_sensitive(self.devices.borrow().has_devices());
        }

        fn recreate_saved_virtual_devices(&self) {
            let devices = self.devices.borrow().saved_devices();
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
        if let Some(id) = self.imp().comment_ids.borrow().get(node) {
            self.imp()
                .comments
                .borrow_mut()
                .set_position(*id, position.x(), position.y());
            return;
        }
        let id = node.pipewire_id();
        let Some(layout_key) = self.imp().topology.borrow().node_key(id).map(str::to_owned) else {
            return;
        };
        self.imp()
            .node_layout
            .borrow_mut()
            .set_position(layout_key, position.x(), position.y());
    }

    fn add_comment_node(&self, id: u32, text: &str, position: Point) -> Node {
        let node = Node::new_comment(text);
        node.comment_buffer().connect_changed(
            glib::clone!(@weak self as manager => move |buffer| {
                let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                manager.imp().comments.borrow_mut().set_text(id, &text);
            }),
        );
        node.connect_local(
            "delete-comment",
            false,
            glib::clone!(@weak self as manager => @default-return None, move |args| {
                let node = args.first().and_then(|arg| arg.get::<Node>().ok())?;
                manager.imp().comment_ids.borrow_mut().remove(&node);
                manager.imp().comments.borrow_mut().remove(id);
                manager.graph().remove_node(&node);
                None
            }),
        );
        self.graph().add_node(node.clone(), None);
        self.graph().set_node_position(&node, &position);
        self.imp().comment_ids.borrow_mut().insert(node.clone(), id);
        node
    }

    fn add_comment(&self) {
        let center = self.graph().visible_center();
        let position = Point::new(center.x() - 110.0, center.y() - 40.0);
        let id = self
            .imp()
            .comments
            .borrow_mut()
            .add(position.x(), position.y());
        let node = self.add_comment_node(id, "", position);
        node.focus_comment();
    }

    fn setup_controls(&self) {
        let window = self.window();
        window
            .add_comment_button()
            .connect_clicked(glib::clone!(@weak self as manager => move |_| manager.add_comment()));
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

    fn request_create_virtual_device(
        &self,
        name: &str,
        kind: VirtualDeviceKind,
        channels: VirtualDeviceChannels,
        save: bool,
    ) {
        let request = self
            .imp()
            .devices
            .borrow_mut()
            .request_create(name, kind, channels, save);
        match request {
            Ok(message) => self.imp().send(message),
            Err(error) => self.show_virtual_device_error(&error),
        }
    }

    fn request_remove_virtual_device(&self, id: u32) {
        self.imp().send(GtkMessage::RemoveVirtualDevice { id });
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

        if res.imp().pw_sender.set(sender).is_err() {
            log::error!("GraphManager already had a PipeWire sender");
        }
        glib::MainContext::default().spawn_local(receive(res.clone(), receiver));

        res.setup_controls();
        res.imp()
            .comments
            .replace(crate::comment_store::CommentStore::load());
        let saved_comments: Vec<_> = res
            .imp()
            .comments
            .borrow()
            .comments()
            .iter()
            .map(|comment| {
                (
                    comment.id,
                    comment.text.clone(),
                    Point::new(comment.x, comment.y),
                )
            })
            .collect();
        for (id, text, position) in saved_comments {
            res.add_comment_node(id, &text, position);
        }
        graph.set_node_position_changed_handler(glib::clone!(@weak res => move |node, position| {
            res.node_position_changed(node, position);
        }));
        res.set_show_monitor_ports(window.show_monitor_ports_button().is_active());
        res.imp().show_meters.set(true);
        res.imp().show_volume_sliders.set(show_volume_sliders);

        res
    }
}
