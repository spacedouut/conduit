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

//! Registry events: binding proxies for new globals and forgetting removed ones.

use std::collections::HashMap;

use adw::glib::clone;
use log::{debug, warn};
use pipewire::{
    device::Device,
    keys,
    link::Link,
    node::Node,
    port::Port,
    registry::{GlobalObject, Registry},
    spa::{param::ParamType, utils::dict::DictRef},
};

use super::{
    callbacks, get_node_name, is_meter, notify, route_device_from_props, state::Item, GtkSender,
    Proxies, ProxyItem, SharedState,
};
use crate::{NodeType, PipewireMessage};

/// Handle a new node being added
pub(super) fn handle_node(
    node: &GlobalObject<&DictRef>,
    sender: &GtkSender,
    registry: &Registry,
    proxies: &Proxies,
    state: &SharedState,
) {
    let node_id = node.id;
    let Some(props) = node.props.as_ref() else {
        warn!("Node {node_id} has no properties, ignoring it");
        return;
    };

    let name = get_node_name(props).to_string();
    let channels = props
        .get("audio.channels")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|channels| *channels > 0)
        .unwrap_or(2);
    let route_device = route_device_from_props(props);
    let layout_key = props
        .get(&keys::NODE_NAME)
        .unwrap_or(name.as_str())
        .to_string();
    let media_class = |class: &str| {
        if class.contains("Sink") || class.contains("Input") {
            Some(NodeType::Input)
        } else if class.contains("Source") || class.contains("Output") {
            Some(NodeType::Output)
        } else {
            None
        }
    };

    let node_type = props
        .get("media.category")
        .and_then(|class| {
            if class.contains("Duplex") {
                None
            } else {
                props.get("media.class").and_then(media_class)
            }
        })
        .or_else(|| props.get("media.class").and_then(media_class));

    state.borrow_mut().insert(node_id, Item::Node);

    notify(
        sender,
        PipewireMessage::NodeAdded {
            id: node_id,
            name,
            layout_key,
            node_type,
            is_meter: is_meter(props),
            is_stream: props
                .get("media.class")
                .is_some_and(|class| class.starts_with("Stream/")),
        },
    );

    // Without a proxy the node still shows up, it just has no volume or media name.
    let proxy: Node = match registry.bind(node) {
        Ok(proxy) => proxy,
        Err(error) => {
            warn!("Failed to bind to node {node_id}: {error}");
            return;
        }
    };
    let listener = proxy
        .add_listener_local()
        .info(clone!(@strong sender, @strong proxies => move |info| {
            callbacks::handle_node_info(info, &sender, &proxies);
        }))
        .param(
            clone!(@strong sender, @strong proxies => move |_, param_type, _, _, param| {
                if param_type == ParamType::Props {
                    callbacks::handle_node_props(param, node_id, &sender, &proxies);
                }
            }),
        )
        .register();
    proxy.subscribe_params(&[ParamType::Props]);
    proxy.enum_params(0, Some(ParamType::Props), 0, u32::MAX);

    proxies.borrow_mut().insert(
        node_id,
        ProxyItem::Node {
            proxy,
            _listener: listener,
            channels,
            channel_volumes: Vec::new(),
            route_device,
        },
    );
}

/// Track device routes so hardware volume can be set the way desktop mixers do.
pub(super) fn handle_device(
    device: &GlobalObject<&DictRef>,
    registry: &Registry,
    proxies: &Proxies,
) {
    let device_id = device.id;
    let proxy: Device = match registry.bind(device) {
        Ok(proxy) => proxy,
        Err(error) => {
            warn!("Failed to bind to device {device_id}: {error}");
            return;
        }
    };
    let listener = proxy
        .add_listener_local()
        .param(
            clone!(@strong proxies => move |_, param_type, _, _, param| {
                if param_type == ParamType::Route {
                    callbacks::handle_device_route(device_id, param, &proxies);
                }
            }),
        )
        .register();
    proxy.subscribe_params(&[ParamType::Route]);
    proxy.enum_params(0, Some(ParamType::Route), 0, u32::MAX);

    proxies.borrow_mut().insert(
        device_id,
        ProxyItem::Device {
            proxy,
            _listener: listener,
            routes: HashMap::new(),
        },
    );
}

/// Handle a new port being added. The GTK thread hears about it once the
/// port's info arrives.
pub(super) fn handle_port(
    port: &GlobalObject<&DictRef>,
    sender: &GtkSender,
    registry: &Registry,
    proxies: &Proxies,
    state: &SharedState,
) {
    let port_id = port.id;
    let proxy: Port = match registry.bind(port) {
        Ok(proxy) => proxy,
        Err(error) => {
            warn!("Failed to bind to port {port_id}: {error}");
            return;
        }
    };
    let listener = proxy
        .add_listener_local()
        .info(
            clone!(@strong proxies, @strong state, @strong sender => move |info| {
                callbacks::handle_port_info(info, &proxies, &state, &sender);
            }),
        )
        .param(clone!(@strong sender => move |_, param_id, _, _, param| {
            if param_id == ParamType::EnumFormat {
                callbacks::handle_port_enum_format(port_id, param, &sender)
            }
        }))
        .register();

    proxies.borrow_mut().insert(
        port_id,
        ProxyItem::Port {
            proxy,
            _listener: listener,
        },
    );
}

/// Handle a new link being added. The GTK thread hears about it once the
/// link's info arrives.
pub(super) fn handle_link(
    link: &GlobalObject<&DictRef>,
    sender: &GtkSender,
    registry: &Registry,
    proxies: &Proxies,
    state: &SharedState,
) {
    debug!(
        "New link (id:{}) appeared, setting up info listener.",
        link.id
    );

    let proxy: Link = match registry.bind(link) {
        Ok(proxy) => proxy,
        Err(error) => {
            warn!("Failed to bind to link {}: {error}", link.id);
            return;
        }
    };
    let listener = proxy
        .add_listener_local()
        .info(clone!(@strong state, @strong sender => move |info| {
            callbacks::handle_link_info(info, &state, &sender);
        }))
        .register();

    proxies.borrow_mut().insert(
        link.id,
        ProxyItem::Link {
            _proxy: proxy,
            _listener: listener,
        },
    );
}

/// Forget a global that went away and tell the GTK thread about it.
pub(super) fn handle_global_remove(
    id: u32,
    sender: &GtkSender,
    proxies: &Proxies,
    state: &SharedState,
) {
    let removed = state.borrow_mut().remove(id);
    if let Some(item) = removed {
        notify(
            sender,
            match item {
                Item::Node => PipewireMessage::NodeRemoved { id },
                Item::Port { node_id } => PipewireMessage::PortRemoved { id, node_id },
                Item::Link { .. } => PipewireMessage::LinkRemoved { id },
            },
        );
    } else if !matches!(proxies.borrow().get(&id), Some(ProxyItem::Device { .. })) {
        warn!("Attempted to remove item with id {id} that is not saved in state");
    }

    proxies.borrow_mut().remove(&id);
}
