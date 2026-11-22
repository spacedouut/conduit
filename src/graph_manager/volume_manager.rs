// SPDX-License-Identifier: GPL-3.0-only

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use crate::{node_volume_store::NodeVolumeStore, virtual_device_store::Store};

/// Volume sliders use the cubic scale desktop mixers show, so 50% here
/// matches 50% in pavucontrol or GNOME Settings.
pub(super) fn slider_from_linear(volume: f32) -> f64 {
    (volume.max(0.0) as f64).cbrt()
}

pub(super) fn linear_from_slider(value: f64) -> f32 {
    value.max(0.0).powi(3) as f32
}

/// How long after a node's first volume report a saved volume keeps winning.
/// A session manager applies the volume it remembers just after a node
/// appears, which can be before or after that first report.
const RESTORE_WINDOW: Duration = Duration::from_secs(2);

/// Restores saved volumes when nodes appear and remembers volume changes.
///
/// Virtual devices keep their volume with their definition; every other node
/// the user changed the volume of in Conduit is remembered by `node.name`.
pub(super) struct VolumeManager {
    /// Nodes whose reported volume should be replaced by a saved one, and
    /// until when; the time is set by the first report.
    pending_restores: HashMap<u32, Option<Instant>>,
    reported_volumes: HashMap<u32, f32>,
    saved: NodeVolumeStore,
    /// Whether `saved` changed since it was last written.
    dirty: bool,
}

impl Default for VolumeManager {
    fn default() -> Self {
        Self {
            pending_restores: HashMap::new(),
            reported_volumes: HashMap::new(),
            saved: NodeVolumeStore::load(),
            dirty: false,
        }
    }
}

impl VolumeManager {
    fn saved_volume(&self, key: &str, devices: &Store) -> Option<f32> {
        devices.volume(key).or_else(|| self.saved.volume(key))
    }

    fn remember(&mut self, key: &str, volume: f32, devices: &mut Store) {
        if devices.contains_node(key) {
            devices.set_volume(key, volume);
        } else if self.saved.set_volume(key, volume) {
            self.dirty = true;
        }
    }

    pub fn node_added(&mut self, id: u32, key: &str, devices: &Store) {
        if self.saved_volume(key, devices).is_some() {
            self.pending_restores.insert(id, None);
        }
    }

    pub fn node_rekeyed(
        &mut self,
        id: u32,
        key: &str,
        devices: &mut Store,
        now: Instant,
    ) -> Option<f32> {
        self.pending_restores.remove(&id);
        self.node_added(id, key, devices);
        let volume = self.reported_volumes.get(&id).copied()?;
        self.remote_volume_changed(id, key, volume, devices, now)
    }

    /// The user moved a volume slider.
    pub fn user_set_volume(&mut self, id: u32, key: &str, volume: f32, devices: &mut Store) {
        // The user's choice wins over a restore that hasn't happened yet.
        self.pending_restores.remove(&id);
        self.remember(key, volume, devices);
    }

    /// Handle a volume reported by PipeWire. Returns a saved volume that
    /// should be applied to the node instead.
    pub fn remote_volume_changed(
        &mut self,
        id: u32,
        key: &str,
        volume: f32,
        devices: &mut Store,
        now: Instant,
    ) -> Option<f32> {
        self.reported_volumes.insert(id, volume);
        if let Some(until) = self.pending_restores.get(&id).copied() {
            let saved = self.saved_volume(key, devices);
            if saved.is_none() || until.is_some_and(|until| now >= until) {
                self.pending_restores.remove(&id);
            } else {
                self.pending_restores
                    .insert(id, Some(until.unwrap_or(now + RESTORE_WINDOW)));
                if let Some(saved) = saved.filter(|saved| (saved - volume).abs() > 0.0001) {
                    log::info!("Restoring volume {saved} on {key}");
                    return Some(saved);
                }
            }
        }
        // Keep up with changes made elsewhere (another mixer, the session
        // manager) to volumes Conduit remembers, so a restart restores the
        // latest one rather than an old one.
        if devices.contains_node(key) || self.saved.contains(key) {
            self.remember(key, volume, devices);
        }
        None
    }

    /// Write remembered volumes if they changed.
    pub fn save(&mut self) {
        if std::mem::take(&mut self.dirty) {
            self.saved.save();
        }
    }

    pub fn needs_save(&self) -> bool {
        self.dirty
    }

    pub fn node_removed(&mut self, id: u32) {
        self.pending_restores.remove(&id);
        self.reported_volumes.remove(&id);
    }

    pub fn clear(&mut self) {
        self.pending_restores.clear();
        self.reported_volumes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> VolumeManager {
        VolumeManager {
            pending_restores: HashMap::new(),
            reported_volumes: HashMap::new(),
            saved: NodeVolumeStore::default(),
            dirty: false,
        }
    }

    #[test]
    fn rekey_before_first_report_preserves_unity_restore() {
        let mut manager = manager();
        let mut devices = Store::default();
        let now = Instant::now();
        manager.saved.set_volume("TestApp", 1.0);
        manager.node_added(1, "TestApp#3", &devices);
        assert_eq!(manager.node_rekeyed(1, "TestApp", &mut devices, now), None);
        assert!(manager.pending_restores.contains_key(&1));
        assert_eq!(
            manager.remote_volume_changed(1, "TestApp", 0.25, &mut devices, now),
            Some(1.0)
        );
        assert_eq!(manager.saved.volume("TestApp"), Some(1.0));
    }

    #[test]
    fn rekey_after_report_restores_using_reported_volume() {
        let mut manager = manager();
        let mut devices = Store::default();
        let now = Instant::now();
        manager.saved.set_volume("TestApp", 1.0);
        assert_eq!(
            manager.remote_volume_changed(1, "TestApp#3", 0.25, &mut devices, now),
            None
        );
        assert_eq!(
            manager.node_rekeyed(1, "TestApp", &mut devices, now),
            Some(1.0)
        );
        manager.node_removed(1);
        assert_eq!(manager.node_rekeyed(1, "TestApp", &mut devices, now), None);
        assert!(manager.pending_restores.contains_key(&1));
        manager.clear();
        assert!(manager.reported_volumes.is_empty());
        assert!(manager.pending_restores.is_empty());
    }

    #[test]
    fn rekey_without_saved_volume_does_not_restore() {
        let mut manager = manager();
        let mut devices = Store::default();
        let now = Instant::now();
        manager.remote_volume_changed(1, "TestApp#3", 0.25, &mut devices, now);
        assert_eq!(manager.node_rekeyed(1, "TestApp", &mut devices, now), None);
        assert!(!manager.pending_restores.contains_key(&1));
        assert_eq!(manager.saved.volume("TestApp"), None);
    }

    #[test]
    fn saved_volume_wins_over_one_applied_just_after_the_first_report() {
        let mut manager = manager();
        let mut devices = Store::default();
        let now = Instant::now();
        manager.saved.set_volume("TestApp", 1.0);
        manager.node_added(1, "TestApp", &devices);
        assert_eq!(
            manager.remote_volume_changed(1, "TestApp", 1.0, &mut devices, now),
            None
        );
        let soon = now + RESTORE_WINDOW / 2;
        assert_eq!(
            manager.remote_volume_changed(1, "TestApp", 0.25, &mut devices, soon),
            Some(1.0)
        );
        assert_eq!(manager.saved.volume("TestApp"), Some(1.0));

        let later = now + RESTORE_WINDOW;
        assert_eq!(
            manager.remote_volume_changed(1, "TestApp", 0.25, &mut devices, later),
            None
        );
        assert_eq!(manager.saved.volume("TestApp"), Some(0.25));
    }

    #[test]
    fn user_volume_ends_the_restore() {
        let mut manager = manager();
        let mut devices = Store::default();
        let now = Instant::now();
        manager.saved.set_volume("TestApp", 1.0);
        manager.node_added(1, "TestApp", &devices);
        manager.remote_volume_changed(1, "TestApp", 1.0, &mut devices, now);
        manager.user_set_volume(1, "TestApp", 0.5, &mut devices);
        assert_eq!(
            manager.remote_volume_changed(1, "TestApp", 0.25, &mut devices, now),
            None
        );
        assert_eq!(manager.saved.volume("TestApp"), Some(0.25));
    }

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
