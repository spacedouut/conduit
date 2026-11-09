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

//! Changes to the PipeWire graph requested by the GTK thread.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use log::{info, warn};
use pipewire::{
    core::Core,
    link::Link,
    node::Node,
    properties::properties,
    registry::Registry,
    spa::{
        param::ParamType,
        pod::{Object, Pod, Property, Value, ValueArray},
        sys as spa_sys,
    },
};

use super::{notify, serialize_pod, GtkSender, Proxies, ProxyItem, SharedState};
use crate::{PipewireMessage, VirtualDeviceChannels, VirtualDeviceKind};

/// Virtual device nodes owned by this session, by Conduit's request id.
pub(super) type VirtualDevices = Rc<RefCell<HashMap<u32, Node>>>;

pub(super) fn set_node_volume(id: u32, volume: f32, proxies: &Proxies) {
    let proxies = proxies.borrow();
    let Some(ProxyItem::Node {
        proxy: node,
        channels,
        channel_volumes,
        route_device,
        ..
    }) = proxies.get(&id)
    else {
        warn!("Cannot set volume for unknown node {id}");
        return;
    };

    let volume = volume.max(0.0);
    let loudest = channel_volumes.iter().copied().fold(0.0f32, f32::max);
    let channel_volumes: Vec<f32> = if loudest > 0.0 {
        channel_volumes
            .iter()
            .map(|channel| channel / loudest * volume)
            .collect()
    } else {
        vec![volume; channel_volumes.len().max(*channels)]
    };
    // Only channelVolumes is touched: the scalar volume multiplies with it, and
    // desktop mixers (pavucontrol, wpctl, GNOME/KDE) all work on channelVolumes.
    let props = |id| Object {
        type_: spa_sys::SPA_TYPE_OBJECT_Props,
        id,
        properties: vec![Property::new(
            spa_sys::SPA_PROP_channelVolumes,
            Value::ValueArray(ValueArray::Float(channel_volumes.clone())),
        )],
    };

    // Hardware nodes get their volume from the device route. Setting it there
    // (like pipewire-pulse and wpctl do) uses the hardware mixer and lets
    // WirePlumber remember it.
    if let Some((device_id, profile_device)) = route_device {
        if let Some(ProxyItem::Device { proxy, routes, .. }) = proxies.get(device_id) {
            if let Some(route_index) = routes.get(profile_device) {
                let route = Value::Object(Object {
                    type_: spa_sys::SPA_TYPE_OBJECT_ParamRoute,
                    id: spa_sys::SPA_PARAM_Route,
                    properties: vec![
                        Property::new(spa_sys::SPA_PARAM_ROUTE_index, Value::Int(*route_index)),
                        Property::new(spa_sys::SPA_PARAM_ROUTE_device, Value::Int(*profile_device)),
                        Property::new(
                            spa_sys::SPA_PARAM_ROUTE_props,
                            Value::Object(props(spa_sys::SPA_PARAM_Route)),
                        ),
                        Property::new(spa_sys::SPA_PARAM_ROUTE_save, Value::Bool(true)),
                    ],
                });
                if let Some(pod) = serialize_pod(&route).as_deref().and_then(Pod::from_bytes) {
                    proxy.set_param(ParamType::Route, 0, pod);
                    return;
                }
            }
        }
    }

    let Some(bytes) = serialize_pod(&Value::Object(props(spa_sys::SPA_PARAM_Props))) else {
        warn!("Could not build volume parameter for node {id}");
        return;
    };
    if let Some(pod) = Pod::from_bytes(&bytes) {
        node.set_param(ParamType::Props, 0, pod);
    }
}

pub(super) fn disconnect_node(node_id: u32, registry: &Registry, state: &SharedState) {
    for link_id in state.borrow().link_ids_for_node(node_id) {
        info!("Requesting removal of link {link_id} from node {node_id}");
        registry.destroy_global(link_id);
    }
}

/// Create an audio adapter backed by PipeWire's null-audio node.
///
/// A virtual output accepts audio from applications and exposes monitor output
/// ports for routing. A virtual input exposes ports that can be fed by the graph.
#[allow(clippy::too_many_arguments)]
pub(super) fn create_virtual_device(
    id: u32,
    name: String,
    node_name: String,
    kind: VirtualDeviceKind,
    channels: VirtualDeviceChannels,
    core: &Core,
    virtual_devices: &VirtualDevices,
    sender: &GtkSender,
) {
    let audio_position = match channels {
        VirtualDeviceChannels::Mono => "MONO",
        VirtualDeviceChannels::Stereo => "FL,FR",
    };

    let result = match kind {
        VirtualDeviceKind::Sink => core.create_object::<Node>(
            "adapter",
            &properties! {
                "factory.name" => "support.null-audio-sink",
                "node.name" => node_name.as_str(),
                "node.description" => name.as_str(),
                "media.class" => "Audio/Sink",
                "audio.position" => audio_position,
                "node.virtual" => "true",
                "node.autoconnect" => "false",
                "monitor.channel-volumes" => "true",
                "application.name" => "Conduit"
            },
        ),
        VirtualDeviceKind::Source => core.create_object::<Node>(
            "adapter",
            &properties! {
                "factory.name" => "support.null-audio-sink",
                "node.name" => node_name.as_str(),
                "node.description" => name.as_str(),
                "media.class" => "Audio/Source/Virtual",
                "audio.position" => audio_position,
                "node.virtual" => "true",
                "node.autoconnect" => "false",
                "monitor.passthrough" => "true",
                "application.name" => "Conduit"
            },
        ),
    };

    match result {
        Ok(node) => {
            info!("Created virtual audio device '{name}'");
            virtual_devices.borrow_mut().insert(id, node);
            notify(
                sender,
                PipewireMessage::VirtualDeviceCreated {
                    id,
                    name,
                    kind,
                    channels,
                },
            );
        }
        Err(error) => {
            warn!("Failed to create virtual audio device: {error}");
            notify(
                sender,
                PipewireMessage::VirtualDeviceError {
                    message: format!("Could not create the virtual device: {error}"),
                },
            );
        }
    }
}

pub(super) fn remove_virtual_device(
    id: u32,
    core: &Core,
    virtual_devices: &VirtualDevices,
    sender: &GtkSender,
) {
    let Some(node) = virtual_devices.borrow_mut().remove(&id) else {
        notify(
            sender,
            PipewireMessage::VirtualDeviceError {
                message: "That virtual device is no longer available.".to_string(),
            },
        );
        return;
    };

    match core.destroy_object(node) {
        Ok(_) => notify(sender, PipewireMessage::VirtualDeviceRemoved { id }),
        Err(error) => {
            warn!("Failed to remove virtual audio device: {error}");
            notify(
                sender,
                PipewireMessage::VirtualDeviceError {
                    message: format!("Could not remove the virtual device: {error}"),
                },
            );
        }
    }
}

/// Toggle a link between the two specified ports.
pub(super) fn toggle_link(
    port_from: u32,
    port_to: u32,
    core: &Core,
    registry: &Registry,
    state: &SharedState,
) {
    let link_id = state.borrow().get_link_id(port_from, port_to);
    if let Some(id) = link_id {
        info!("Requesting removal of link with id {}", id);
        // A failure comes back asynchronously as a core error, which is logged.
        registry.destroy_global(id);
    } else {
        create_link(port_from, port_to, core, state);
    }
}

pub(super) fn create_link(port_from: u32, port_to: u32, core: &Core, state: &SharedState) {
    info!(
        "Requesting creation of link from port id:{} to port id:{}",
        port_from, port_to
    );

    let state = state.borrow();
    let (Some(node_from), Some(node_to)) = (
        state.get_node_of_port(port_from),
        state.get_node_of_port(port_to),
    ) else {
        warn!("Cannot link unknown ports {port_from} -> {port_to}");
        return;
    };

    if let Err(e) = core.create_object::<Link>(
        "link-factory",
        &properties! {
            "link.output.node" => node_from.to_string(),
            "link.output.port" => port_from.to_string(),
            "link.input.node" => node_to.to_string(),
            "link.input.port" => port_to.to_string(),
            "object.linger" => "1"
        },
    ) {
        warn!("Failed to create link: {}", e);
    }
}
