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

//! Info and param events of the node, port, link and device proxies we bound.

use log::{debug, error, warn};
use pipewire::{
    keys,
    link::{LinkChangeMask, LinkInfoRef, LinkState},
    node::NodeInfoRef,
    port::{PortChangeMask, PortInfoRef},
    spa::{
        param::{ParamInfoFlags, ParamType},
        pod::{Pod, Value, ValueArray},
        sys as spa_sys,
    },
};

use super::{
    deserialize_object, get_link_media_type, get_node_name, is_meter, notify, property_is_true,
    route_device_from_props, state::Item, GtkSender, Proxies, ProxyItem, SharedState,
};
use crate::{MediaType, PipewireMessage};

pub(super) fn handle_node_info(info: &NodeInfoRef, sender: &GtkSender, proxies: &Proxies) {
    debug!("Received node info: {:?}", info);

    let id = info.id();
    let mut proxies = proxies.borrow_mut();
    let Some(ProxyItem::Node { route_device, .. }) = proxies.get_mut(&id) else {
        error!("Received info on unknown node with id {id}");
        return;
    };

    let Some(props) = info.props() else {
        warn!("Info for node {id} has no properties");
        return;
    };
    // card.profile.device isn't in the registry's global props, only here.
    if let Some(route) = route_device_from_props(props) {
        *route_device = Some(route);
    }
    if let Some(media_name) = props.get(&keys::MEDIA_NAME) {
        notify(
            sender,
            PipewireMessage::NodeNameChanged {
                id,
                name: get_node_name(props).to_string(),
                media_name: media_name.to_string(),
            },
        );
    }
}

pub(super) fn handle_node_props(
    param: Option<&Pod>,
    id: u32,
    sender: &GtkSender,
    proxies: &Proxies,
) {
    let Some(object) = deserialize_object(param) else {
        return;
    };
    let find = |key| {
        object
            .properties
            .iter()
            .find(|property| property.key == key)
            .map(|property| &property.value)
    };
    // The effective gain is channelVolumes. The scalar volume usually sits at
    // 1.0 on session-managed nodes, so only fall back to it when needed.
    let volume = match find(spa_sys::SPA_PROP_channelVolumes) {
        Some(Value::ValueArray(ValueArray::Float(volumes))) if !volumes.is_empty() => {
            if let Some(ProxyItem::Node {
                channel_volumes, ..
            }) = proxies.borrow_mut().get_mut(&id)
            {
                channel_volumes.clone_from(volumes);
            }
            volumes.iter().copied().fold(0.0f32, f32::max)
        }
        _ => match find(spa_sys::SPA_PROP_volume) {
            Some(Value::Float(volume)) => *volume,
            _ => return,
        },
    };
    notify(sender, PipewireMessage::NodeVolumeChanged { id, volume });
}

/// Remember a device's active route for each `card.profile.device`.
pub(super) fn handle_device_route(device_id: u32, param: Option<&Pod>, proxies: &Proxies) {
    let Some(object) = deserialize_object(param) else {
        return;
    };
    let int = |key| {
        object
            .properties
            .iter()
            .find_map(|property| match property.value {
                Value::Int(value) if property.key == key => Some(value),
                _ => None,
            })
    };
    let (Some(index), Some(profile_device)) = (
        int(spa_sys::SPA_PARAM_ROUTE_index),
        int(spa_sys::SPA_PARAM_ROUTE_device),
    ) else {
        return;
    };
    if let Some(ProxyItem::Device { routes, .. }) = proxies.borrow_mut().get_mut(&device_id) {
        routes.insert(profile_device, index);
    }
}

pub(super) fn handle_port_info(
    info: &PortInfoRef,
    proxies: &Proxies,
    state: &SharedState,
    sender: &GtkSender,
) {
    debug!("Received port info: {:?}", info);

    let id = info.id();
    let proxies = proxies.borrow();
    let Some(ProxyItem::Port { proxy, .. }) = proxies.get(&id) else {
        error!("Received info on unknown port with id {id}");
        return;
    };

    let mut state = state.borrow_mut();

    if let Some(Item::Port { .. }) = state.get(id) {
        // Info was an update, figure out if we should notify the GTK thread
        if info.change_mask().contains(PortChangeMask::PARAMS) {
            // TODO: React to param changes
        }
        return;
    }

    // First time we get info. We can now notify the gtk thread of a new port.
    let Some(props) = info.props() else {
        warn!("Info for port {id} has no properties");
        return;
    };
    let name = props.get("port.name").unwrap_or_default().to_string();
    let Some(node_id) = props.get("node.id").and_then(|id| id.parse::<u32>().ok()) else {
        warn!("Port {id} has no valid node.id property, ignoring it");
        return;
    };

    state.insert(id, Item::Port { node_id });

    let params = info.params();
    let enum_format_info = params
        .iter()
        .find(|param| param.id() == ParamType::EnumFormat);
    if let Some(enum_format_info) = enum_format_info {
        if enum_format_info.flags().contains(ParamInfoFlags::READ) {
            proxy.enum_params(0, Some(ParamType::EnumFormat), 0, u32::MAX);
        }
    }

    notify(
        sender,
        PipewireMessage::PortAdded {
            id,
            node_id,
            name,
            direction: info.direction(),
            is_monitor: property_is_true(props, "port.monitor"),
            is_meter: is_meter(props),
        },
    );
}

pub(super) fn handle_port_enum_format(port_id: u32, param: Option<&Pod>, sender: &GtkSender) {
    let media_type = param
        .and_then(|param| pipewire::spa::param::format_utils::parse_format(param).ok())
        .map(|(media_type, _media_subtype)| media_type)
        .unwrap_or(MediaType::Unknown);

    notify(
        sender,
        PipewireMessage::PortFormatChanged {
            id: port_id,
            media_type,
        },
    );
}

pub(super) fn handle_link_info(info: &LinkInfoRef, state: &SharedState, sender: &GtkSender) {
    debug!("Received link info: {:?}", info);

    let id = info.id();

    let mut state = state.borrow_mut();
    if let Some(Item::Link { .. }) = state.get(id) {
        // Info was an update - figure out if we should notify the gtk thread
        if info.change_mask().contains(LinkChangeMask::STATE) {
            notify(
                sender,
                PipewireMessage::LinkStateChanged {
                    id,
                    active: matches!(info.state(), LinkState::Active),
                },
            );
        }
        if info.change_mask().contains(LinkChangeMask::FORMAT) {
            notify(
                sender,
                PipewireMessage::LinkFormatChanged {
                    id,
                    media_type: get_link_media_type(info),
                },
            );
        }
    } else {
        // First time we get info. We can now notify the gtk thread of a new link.
        let port_from = info.output_port_id();
        let port_to = info.input_port_id();

        state.insert(id, Item::Link { port_from, port_to });

        notify(
            sender,
            PipewireMessage::LinkAdded {
                id,
                port_from,
                port_to,
                active: matches!(info.state(), LinkState::Active),
                media_type: get_link_media_type(info),
            },
        );
    }
}
