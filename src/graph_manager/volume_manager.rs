// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashSet;

use crate::virtual_device_store::Store;

/// Volume sliders use the cubic scale desktop mixers show, so 50% here
/// matches 50% in pavucontrol or GNOME Settings.
pub(super) fn slider_from_linear(volume: f32) -> f64 {
    (volume.max(0.0) as f64).cbrt()
}

pub(super) fn linear_from_slider(value: f64) -> f32 {
    value.max(0.0).powi(3) as f32
}

/// Restores saved volumes when nodes appear and remembers volume changes.
#[derive(Default)]
pub(super) struct VolumeManager {
    /// Nodes whose first reported volume should be replaced by a saved one.
    pending_restores: HashSet<u32>,
}

impl VolumeManager {
    pub fn node_added(&mut self, id: u32, key: &str, devices: &Store) {
        if devices.volume(key).is_some() {
            self.pending_restores.insert(id);
        }
    }

    /// Handle a volume reported by PipeWire. Returns a saved volume that
    /// should be applied to the node instead.
    pub fn remote_volume_changed(
        &mut self,
        id: u32,
        key: &str,
        volume: f32,
        devices: &mut Store,
    ) -> Option<f32> {
        if self.pending_restores.remove(&id) {
            if let Some(saved) = devices
                .volume(key)
                .filter(|saved| (saved - volume).abs() > 0.0001)
            {
                log::info!("Restoring volume {saved} on {key}");
                return Some(saved);
            }
        }
        if devices.contains_node(key) {
            devices.set_volume(key, volume);
        }
        None
    }

    pub fn node_removed(&mut self, id: u32) {
        self.pending_restores.remove(&id);
    }

    pub fn clear(&mut self) {
        self.pending_restores.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_scale_round_trips() {
        for volume in [0.0f32, 0.125, 0.5, 1.0, 1.5] {
            let back = linear_from_slider(slider_from_linear(volume));
            assert!((back - volume).abs() < 1e-5, "{volume} -> {back}");
        }
        assert!((slider_from_linear(0.125) - 0.5).abs() < 1e-9);
        assert_eq!(linear_from_slider(-1.0), 0.0);
    }
}
