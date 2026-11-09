// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;

use crate::{
    virtual_device_store::{Definition, Store},
    GtkMessage, VirtualDeviceChannels, VirtualDeviceKind,
};

#[derive(Clone)]
pub(super) struct VirtualDevice {
    pub name: String,
    pub kind: VirtualDeviceKind,
    pub channels: VirtualDeviceChannels,
}

impl VirtualDevice {
    pub fn definition(&self) -> Definition {
        Definition {
            name: self.name.clone(),
            kind: self.kind,
            channels: self.channels,
        }
    }
}

/// Virtual devices owned by this Conduit session, and the saved ones that are
/// recreated on every launch.
#[derive(Default)]
pub(super) struct DeviceManager {
    live: HashMap<u32, VirtualDevice>,
    pub store: Store,
    /// Whether to save each requested device, and its node name.
    pending_saves: HashMap<u32, (bool, String)>,
    next_id: u32,
    connected: bool,
}

impl DeviceManager {
    pub fn connected(&mut self) {
        self.connected = true;
    }

    /// PipeWire drops requests it gets while disconnected, and the devices
    /// this session owned are gone with the connection.
    pub fn disconnected(&mut self) {
        self.connected = false;
        self.pending_saves.clear();
        self.live.clear();
    }

    pub fn request_failed(&mut self) {
        self.pending_saves.clear();
    }

    /// Build the request for a new virtual device, or explain why it can't be made.
    pub fn request_create(
        &mut self,
        name: &str,
        kind: VirtualDeviceKind,
        channels: VirtualDeviceChannels,
        save: bool,
    ) -> Result<GtkMessage, String> {
        if !self.connected {
            return Err("Not connected to PipeWire.".to_string());
        }
        let definition = Definition {
            name: name.to_string(),
            kind,
            channels,
        };
        let node_name = definition.node_name();
        // A saved device with this exact definition and no live node is a
        // retry; anything else with the same node name would share its state.
        let taken = self
            .store
            .devices()
            .iter()
            .any(|saved| saved.node_name() == node_name && *saved != definition)
            || self
                .pending_saves
                .values()
                .any(|(_, pending)| *pending == node_name)
            || self
                .live
                .values()
                .any(|device| device.definition().node_name() == node_name);
        if save && taken {
            return Err(format!(
                "A virtual {} with a name like \"{name}\" already exists.",
                match kind {
                    VirtualDeviceKind::Sink => "sink",
                    VirtualDeviceKind::Source => "source",
                }
            ));
        }
        let id = self.next_id;
        self.next_id = id.wrapping_add(1);
        self.pending_saves.insert(id, (save, node_name.clone()));
        Ok(GtkMessage::CreateVirtualDevice {
            id,
            name: name.to_string(),
            node_name,
            kind,
            channels,
        })
    }

    pub fn created(&mut self, id: u32, device: VirtualDevice) {
        let save = self.pending_saves.remove(&id).is_some_and(|(save, _)| save);
        if save {
            self.store.add(device.definition());
        }
        self.live.insert(id, device);
    }

    pub fn removed(&mut self, id: u32) {
        if let Some(device) = self.live.remove(&id) {
            self.store.remove(&device.definition());
        }
    }

    pub fn has_devices(&self) -> bool {
        !self.live.is_empty()
    }

    /// Live devices, sorted by name.
    pub fn devices(&self) -> Vec<(u32, VirtualDevice)> {
        let mut devices: Vec<_> = self
            .live
            .iter()
            .map(|(id, device)| (*id, device.clone()))
            .collect();
        devices.sort_by(|left, right| left.1.name.cmp(&right.1.name));
        devices
    }

    pub fn saved_devices(&self) -> Vec<Definition> {
        self.store.devices()
    }
}
