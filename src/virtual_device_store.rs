// SPDX-License-Identifier: GPL-3.0-only

use std::{fs, path::Path, path::PathBuf};

use adw::glib;

use crate::{VirtualDeviceChannels, VirtualDeviceKind};

#[derive(Clone, PartialEq, Eq)]
pub struct Definition {
    pub name: String,
    pub kind: VirtualDeviceKind,
    pub channels: VirtualDeviceChannels,
}

impl Definition {
    /// Stable `node.name` for this device, so layout, volume and links can be
    /// matched across launches.
    pub fn node_name(&self) -> String {
        let slug: String = self
            .name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let slug = slug.trim_matches('_');
        let kind = match self.kind {
            VirtualDeviceKind::Sink => "sink",
            VirtualDeviceKind::Source => "source",
        };
        if slug.is_empty() {
            format!("conduit.virtual.{kind}.{}", encode(&self.name))
        } else {
            format!("conduit.virtual.{kind}.{slug}")
        }
    }
}

/// A link between two ports, identified by `node.name` and `port.name` so it
/// can be found again after the PipeWire ids change.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SavedLink {
    pub output_node: String,
    pub output_port: String,
    pub input_node: String,
    pub input_port: String,
}

impl SavedLink {
    pub fn touches(&self, node_name: &str) -> bool {
        self.output_node == node_name || self.input_node == node_name
    }
}

struct SavedDevice {
    definition: Definition,
    volume: Option<f32>,
}

pub struct Store {
    devices: Vec<SavedDevice>,
    links: Vec<SavedLink>,
    devices_path: PathBuf,
    links_path: PathBuf,
}

impl Default for Store {
    fn default() -> Self {
        Self::load()
    }
}

impl Store {
    pub fn load() -> Self {
        let directory = glib::user_config_dir().join("conduit");
        let devices_path = directory.join("virtual-devices");
        let links_path = directory.join("virtual-device-links");
        Self {
            devices: read_lines(&devices_path)
                .filter_map(|fields| parse_device(&fields))
                .collect(),
            links: read_lines(&links_path)
                .filter_map(|fields| parse_link(&fields))
                .collect(),
            devices_path,
            links_path,
        }
    }

    pub fn devices(&self) -> Vec<Definition> {
        self.devices
            .iter()
            .map(|device| device.definition.clone())
            .collect()
    }

    pub fn contains_node(&self, node_name: &str) -> bool {
        self.devices
            .iter()
            .any(|device| device.definition.node_name() == node_name)
    }

    pub fn add(&mut self, definition: Definition) {
        if !self
            .devices
            .iter()
            .any(|device| device.definition == definition)
        {
            self.devices.push(SavedDevice {
                definition,
                volume: None,
            });
            self.save_devices();
        }
    }

    pub fn remove(&mut self, definition: &Definition) {
        let node_name = definition.node_name();
        self.devices
            .retain(|device| &device.definition != definition);
        self.save_devices();
        if !self.contains_node(&node_name) {
            let count = self.links.len();
            self.links.retain(|link| !link.touches(&node_name));
            if self.links.len() != count {
                self.save_links();
            }
        }
    }

    pub fn volume(&self, node_name: &str) -> Option<f32> {
        self.devices
            .iter()
            .find(|device| device.definition.node_name() == node_name)
            .and_then(|device| device.volume)
    }

    pub fn set_volume(&mut self, node_name: &str, volume: f32) {
        let mut changed = false;
        for device in &mut self.devices {
            if device.definition.node_name() == node_name
                && device
                    .volume
                    .map_or(true, |saved| (saved - volume).abs() > f32::EPSILON)
            {
                device.volume = Some(volume);
                changed = true;
            }
        }
        if changed {
            self.save_devices();
        }
    }

    pub fn links(&self) -> &[SavedLink] {
        &self.links
    }

    pub fn add_link(&mut self, link: SavedLink) {
        if !self.links.contains(&link) {
            self.links.push(link);
            self.save_links();
        }
    }

    pub fn remove_link(&mut self, link: &SavedLink) {
        let count = self.links.len();
        self.links.retain(|saved| saved != link);
        if self.links.len() != count {
            self.save_links();
        }
    }

    fn save_devices(&self) {
        let contents = self
            .devices
            .iter()
            .map(|device| {
                let mut line = format!(
                    "{}\t{}\t{}",
                    encode(&device.definition.name),
                    match device.definition.kind {
                        VirtualDeviceKind::Sink => "sink",
                        VirtualDeviceKind::Source => "source",
                    },
                    match device.definition.channels {
                        VirtualDeviceChannels::Mono => "mono",
                        VirtualDeviceChannels::Stereo => "stereo",
                    }
                );
                if let Some(volume) = device.volume {
                    line.push_str(&format!("\t{volume}"));
                }
                line.push('\n');
                line
            })
            .collect::<String>();
        write_atomically(&self.devices_path, &contents, "virtual devices");
    }

    fn save_links(&self) {
        let contents = self
            .links
            .iter()
            .map(|link| {
                format!(
                    "{}\t{}\t{}\t{}\n",
                    encode(&link.output_node),
                    encode(&link.output_port),
                    encode(&link.input_node),
                    encode(&link.input_port)
                )
            })
            .collect::<String>();
        write_atomically(&self.links_path, &contents, "virtual device links");
    }
}

fn read_lines(path: &Path) -> impl Iterator<Item = Vec<String>> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| line.split('\t').map(str::to_owned).collect())
        .collect::<Vec<_>>()
        .into_iter()
}

fn parse_device(fields: &[String]) -> Option<SavedDevice> {
    let [name, kind, channels, rest @ ..] = fields else {
        return None;
    };
    Some(SavedDevice {
        definition: Definition {
            name: decode(name)?,
            kind: match kind.as_str() {
                "sink" => VirtualDeviceKind::Sink,
                "source" => VirtualDeviceKind::Source,
                _ => return None,
            },
            channels: match channels.as_str() {
                "mono" => VirtualDeviceChannels::Mono,
                "stereo" => VirtualDeviceChannels::Stereo,
                _ => return None,
            },
        },
        volume: rest.first().and_then(|volume| volume.parse().ok()),
    })
}

fn parse_link(fields: &[String]) -> Option<SavedLink> {
    let [output_node, output_port, input_node, input_port] = fields else {
        return None;
    };
    Some(SavedLink {
        output_node: decode(output_node)?,
        output_port: decode(output_port)?,
        input_node: decode(input_node)?,
        input_port: decode(input_port)?,
    })
}

fn write_atomically(path: &Path, contents: &str, what: &str) {
    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(error) = fs::create_dir_all(parent) {
        log::warn!("Could not create config directory for {what}: {error}");
        return;
    }
    let temporary_path = path.with_extension("tmp");
    if let Err(error) =
        fs::write(&temporary_path, contents).and_then(|_| fs::rename(&temporary_path, path))
    {
        log::warn!("Could not save {what}: {error}");
        let _ = fs::remove_file(temporary_path);
    }
}

fn encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode(encoded: &str) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_lines_round_trip_with_and_without_volume() {
        let fields = |line: &str| line.split('\t').map(str::to_owned).collect::<Vec<_>>();
        let old = parse_device(&fields(&format!("{}\tsink\tstereo", encode("Mix")))).unwrap();
        assert_eq!(old.volume, None);
        let new = parse_device(&fields(&format!("{}\tsource\tmono\t0.5", encode("Mic")))).unwrap();
        assert_eq!(new.volume, Some(0.5));
        assert_eq!(new.definition.node_name(), "conduit.virtual.source.mic");
    }

    #[test]
    fn node_names_are_stable() {
        let definition = Definition {
            name: "Podcast mix".into(),
            kind: VirtualDeviceKind::Sink,
            channels: VirtualDeviceChannels::Stereo,
        };
        assert_eq!(definition.node_name(), "conduit.virtual.sink.podcast_mix");
    }
}
