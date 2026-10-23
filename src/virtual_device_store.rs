// SPDX-License-Identifier: GPL-3.0-only

use std::{fs, path::PathBuf};

use adw::glib;

use crate::{VirtualDeviceChannels, VirtualDeviceKind};

#[derive(Clone, PartialEq, Eq)]
pub struct Definition {
    pub name: String,
    pub kind: VirtualDeviceKind,
    pub channels: VirtualDeviceChannels,
}

pub struct Store {
    devices: Vec<Definition>,
    path: PathBuf,
}

impl Default for Store {
    fn default() -> Self {
        Self::load()
    }
}

impl Store {
    pub fn load() -> Self {
        let path = glib::user_config_dir().join("conduit/virtual-devices");
        let devices = fs::read_to_string(&path)
            .ok()
            .into_iter()
            .flat_map(|contents| contents.lines().map(str::to_owned).collect::<Vec<_>>())
            .filter_map(|line| {
                let mut fields = line.split('\t');
                let (Some(name), Some(kind), Some(channels)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    return None;
                };
                Some(Definition {
                    name: decode_name(name)?,
                    kind: match kind {
                        "sink" => VirtualDeviceKind::Sink,
                        "source" => VirtualDeviceKind::Source,
                        _ => return None,
                    },
                    channels: match channels {
                        "mono" => VirtualDeviceChannels::Mono,
                        "stereo" => VirtualDeviceChannels::Stereo,
                        _ => return None,
                    },
                })
            })
            .collect();
        Self { devices, path }
    }

    pub fn devices(&self) -> &[Definition] {
        &self.devices
    }

    pub fn add(&mut self, device: Definition) {
        if !self.devices.contains(&device) {
            self.devices.push(device);
            self.save();
        }
    }

    pub fn remove(&mut self, device: &Definition) {
        self.devices.retain(|saved| saved != device);
        self.save();
    }

    fn save(&self) {
        let Some(parent) = self.path.parent() else {
            return;
        };
        if let Err(error) = fs::create_dir_all(parent) {
            log::warn!("Could not create virtual-device directory: {error}");
            return;
        }
        let contents = self
            .devices
            .iter()
            .map(|device| {
                format!(
                    "{}\t{}\t{}\n",
                    encode_name(&device.name),
                    match device.kind {
                        VirtualDeviceKind::Sink => "sink",
                        VirtualDeviceKind::Source => "source",
                    },
                    match device.channels {
                        VirtualDeviceChannels::Mono => "mono",
                        VirtualDeviceChannels::Stereo => "stereo",
                    }
                )
            })
            .collect::<String>();
        let temporary_path = self.path.with_extension("tmp");
        if let Err(error) = fs::write(&temporary_path, contents)
            .and_then(|_| fs::rename(&temporary_path, &self.path))
        {
            log::warn!("Could not save virtual devices: {error}");
            let _ = fs::remove_file(temporary_path);
        }
    }
}

fn encode_name(name: &str) -> String {
    name.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn decode_name(encoded: &str) -> Option<String> {
    if encoded.len() % 2 != 0 {
        return None;
    }
    String::from_utf8(
        (0..encoded.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&encoded[index..index + 2], 16).ok())
            .collect::<Option<Vec<_>>>()?,
    )
    .ok()
}
