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

/// A request to create a virtual device.
pub(super) struct CreateRequest {
    pub id: u32,
    pub name: String,
    pub node_name: String,
    pub kind: VirtualDeviceKind,
    pub channels: VirtualDeviceChannels,
}

/// The virtual devices this connection manages, by Conduit's request id.
///
/// The nodes linger in PipeWire, so they outlive Conduit and a later launch
/// finds them in the registry. Create requests therefore wait until the
/// registry has been listed once: only then is it known which devices are
/// already there and must be adopted instead of created a second time.
pub(super) struct VirtualDevices<P = Node> {
    registry_listed: bool,
    waiting: Vec<CreateRequest>,
    devices: HashMap<u32, ManagedDevice<P>>,
}

struct ManagedDevice<P> {
    node_name: String,
    /// Kept for devices created on this connection; adopted ones have none.
    _proxy: Option<P>,
}

pub(super) type SharedVirtualDevices = Rc<RefCell<VirtualDevices>>;

impl<P> Default for VirtualDevices<P> {
    fn default() -> Self {
        Self {
            registry_listed: false,
            waiting: Vec::new(),
            devices: HashMap::new(),
        }
    }
}

impl<P> VirtualDevices<P> {
    /// Hand the request back if it can be handled now, or keep it until
    /// [`Self::registry_listed`].
    fn ready(&mut self, request: CreateRequest) -> Option<CreateRequest> {
        if self.registry_listed {
            Some(request)
        } else {
            self.waiting.push(request);
            None
        }
    }

    /// The registry has been listed; returns the requests that were waiting for it.
    fn registry_listed(&mut self) -> Vec<CreateRequest> {
        self.registry_listed = true;
        std::mem::take(&mut self.waiting)
    }

    fn manage(&mut self, id: u32, node_name: String, proxy: Option<P>) {
        self.devices.insert(
            id,
            ManagedDevice {
                node_name,
                _proxy: proxy,
            },
        );
    }

    /// Stop managing a device, returning its `node.name`.
    fn forget(&mut self, id: u32) -> Option<String> {
        self.devices.remove(&id).map(|device| device.node_name)
    }
}

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

/// Create a virtual device, or adopt the one a previous Conduit left running.
pub(super) fn create_virtual_device(
    request: CreateRequest,
    core: &Core,
    state: &SharedState,
    virtual_devices: &SharedVirtualDevices,
    sender: &GtkSender,
) {
    let Some(request) = virtual_devices.borrow_mut().ready(request) else {
        return;
    };
    create_or_adopt_virtual_device(request, core, state, virtual_devices, sender);
}

/// Handle the create requests that arrived before the registry was listed.
pub(super) fn registry_listed(
    core: &Core,
    state: &SharedState,
    virtual_devices: &SharedVirtualDevices,
    sender: &GtkSender,
) {
    let waiting = virtual_devices.borrow_mut().registry_listed();
    for request in waiting {
        create_or_adopt_virtual_device(request, core, state, virtual_devices, sender);
    }
}

/// Create an audio adapter backed by PipeWire's null-audio node, unless a node
/// with the same `node.name` is already there.
///
/// A virtual output accepts audio from applications and exposes monitor output
/// ports for routing. A virtual input exposes ports that can be fed by the graph.
///
/// The node lingers: it stays in PipeWire when Conduit quits or loses its
/// connection, so whatever is routed through it keeps working. It goes away
/// when it is removed in Conduit or when PipeWire restarts.
fn create_or_adopt_virtual_device(
    request: CreateRequest,
    core: &Core,
    state: &SharedState,
    virtual_devices: &SharedVirtualDevices,
    sender: &GtkSender,
) {
    let CreateRequest {
        id,
        name,
        node_name,
        kind,
        channels,
    } = request;
    let created = PipewireMessage::VirtualDeviceCreated {
        id,
        name: name.clone(),
        kind,
        channels,
    };

    if state.borrow().node_id_by_name(&node_name).is_some() {
        info!("Virtual audio device '{name}' is already running");
        virtual_devices.borrow_mut().manage(id, node_name, None);
        notify(sender, created);
        return;
    }

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
                "application.name" => "Conduit",
                "object.linger" => "true",
                // Conduit keeps the volume itself; the session manager's
                // copy is shared by every device and outlives a removed one.
                "state.restore-props" => "false"
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
                "application.name" => "Conduit",
                "object.linger" => "true",
                // Conduit keeps the volume itself; the session manager's
                // copy is shared by every device and outlives a removed one.
                "state.restore-props" => "false"
            },
        ),
    };

    match result {
        Ok(node) => {
            info!("Created virtual audio device '{name}'");
            virtual_devices
                .borrow_mut()
                .manage(id, node_name, Some(node));
            notify(sender, created);
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
    registry: &Registry,
    state: &SharedState,
    virtual_devices: &SharedVirtualDevices,
    sender: &GtkSender,
) {
    let Some(node_name) = virtual_devices.borrow_mut().forget(id) else {
        notify(
            sender,
            PipewireMessage::VirtualDeviceError {
                message: "That virtual device is no longer available.".to_string(),
            },
        );
        return;
    };

    // A lingering node is not tied to the proxy that created it, so it has to
    // be destroyed through the registry. If it is not listed, something else
    // already removed it. A failure comes back as a core error, which is logged.
    if let Some(node_id) = state.borrow().node_id_by_name(&node_name) {
        info!("Removing virtual audio device node {node_id} ({node_name})");
        registry.destroy_global(node_id);
    }
    notify(sender, PipewireMessage::VirtualDeviceRemoved { id });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: u32) -> CreateRequest {
        CreateRequest {
            id,
            name: format!("Device {id}"),
            node_name: format!("conduit.virtual.sink.device_{id}"),
            kind: VirtualDeviceKind::Sink,
            channels: VirtualDeviceChannels::Stereo,
        }
    }

    #[test]
    fn create_requests_wait_for_the_registry_listing() {
        let mut devices = VirtualDevices::<()>::default();
        assert!(devices.ready(request(0)).is_none());
        assert!(devices.ready(request(1)).is_none());

        let waiting: Vec<u32> = devices
            .registry_listed()
            .iter()
            .map(|request| request.id)
            .collect();
        assert_eq!(waiting, [0, 1]);

        assert_eq!(devices.ready(request(2)).map(|request| request.id), Some(2));
        assert!(devices.registry_listed().is_empty());
    }

    #[test]
    fn managed_devices_are_forgotten_once() {
        let mut devices = VirtualDevices::<()>::default();
        devices.manage(3, "conduit.virtual.sink.mix".to_string(), None);
        devices.manage(4, "conduit.virtual.source.mic".to_string(), Some(()));

        assert_eq!(
            devices.forget(3).as_deref(),
            Some("conduit.virtual.sink.mix")
        );
        assert_eq!(devices.forget(3), None);
        assert_eq!(
            devices.forget(4).as_deref(),
            Some("conduit.virtual.source.mic")
        );
    }
}
