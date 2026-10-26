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

mod application;
mod autostart;
mod graph_manager;
mod node_filter;
mod node_layout;
mod pipewire_connection;
mod tray;
mod ui;
mod virtual_device_store;
mod volume_slider_setting;

use adw::{gtk, prelude::*};
use pipewire::spa::{param::format::MediaType, utils::Direction};

/// Messages sent by the GTK thread to notify the pipewire thread.
#[derive(Debug, Clone)]
pub enum GtkMessage {
    /// Toggle a link between the two specified ports.
    ToggleLink {
        port_from: u32,
        port_to: u32,
    },
    /// Create a link between the two ports unless one already exists.
    CreateLink {
        port_from: u32,
        port_to: u32,
    },
    /// Set the linear volume of a node, keeping its channel balance.
    SetNodeVolume {
        node_id: u32,
        volume: f32,
    },
    DisconnectNode {
        node_id: u32,
    },
    /// Create a virtual audio endpoint owned by this Conduit session.
    CreateVirtualDevice {
        id: u32,
        name: String,
        node_name: String,
        kind: VirtualDeviceKind,
        channels: VirtualDeviceChannels,
    },
    /// Remove a virtual audio endpoint created by this Conduit session.
    RemoveVirtualDevice {
        id: u32,
    },
    /// Connect to PipeWire service.
    Connect(Option<String>),
    /// Quit the event loop and let the thread finish.
    Terminate,
}

/// Messages sent by the pipewire thread to notify the GTK thread.
#[derive(Debug, Clone)]
pub enum PipewireMessage {
    NodeAdded {
        id: u32,
        name: String,
        layout_key: String,
        node_type: Option<NodeType>,
        is_meter: bool,
        /// An application stream, routed by the session manager.
        is_stream: bool,
    },
    NodeNameChanged {
        id: u32,
        name: String,
        media_name: String,
    },
    /// The loudest channel's linear volume.
    NodeVolumeChanged {
        id: u32,
        volume: f32,
    },
    PortAdded {
        id: u32,
        node_id: u32,
        name: String,
        direction: Direction,
        is_monitor: bool,
        is_meter: bool,
    },
    PortFormatChanged {
        id: u32,
        media_type: MediaType,
    },
    LinkAdded {
        id: u32,
        port_from: u32,
        port_to: u32,
        active: bool,
        media_type: MediaType,
    },
    LinkStateChanged {
        id: u32,
        active: bool,
    },
    LinkFormatChanged {
        id: u32,
        media_type: MediaType,
    },
    NodeRemoved {
        id: u32,
    },
    PortRemoved {
        id: u32,
        node_id: u32,
    },
    LinkRemoved {
        id: u32,
    },
    VirtualDeviceCreated {
        id: u32,
        name: String,
        kind: VirtualDeviceKind,
        channels: VirtualDeviceChannels,
    },
    VirtualDeviceRemoved {
        id: u32,
    },
    VirtualDeviceError {
        message: String,
    },
    Connecting,
    Connected,
    Disconnected,
}

#[derive(Debug, Clone)]
pub enum NodeType {
    Input,
    Output,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VirtualDeviceKind {
    /// A sink applications can play into.
    Sink,
    /// A source applications can record from.
    Source,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VirtualDeviceChannels {
    Mono,
    Stereo,
}

#[derive(Debug, Clone)]
pub struct PipewireLink {
    pub node_from: u32,
    pub port_from: u32,
    pub node_to: u32,
    pub port_to: u32,
}

static GLIB_LOGGER: glib::GlibLogger = glib::GlibLogger::new(
    glib::GlibLoggerFormat::Structured,
    glib::GlibLoggerDomain::CrateTarget,
);

fn init_glib_logger() {
    log::set_logger(&GLIB_LOGGER).expect("Failed to set logger");

    // Glib does not have a "Trace" log level, so only print messages "Debug" or higher priority.
    log::set_max_level(log::LevelFilter::Debug);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_glib_logger();
    gtk::init()?;

    // Aquire main context so that we can attach the gtk channel later.
    let ctx = glib::MainContext::default();
    let _guard = ctx.acquire().unwrap();

    // Start the pipewire thread with channels in both directions.

    let (gtk_sender, gtk_receiver) = async_channel::unbounded();
    let (pw_sender, pw_receiver) = pipewire::channel::channel();
    let pw_thread =
        std::thread::spawn(move || pipewire_connection::thread_main(gtk_sender, pw_receiver));

    let app = application::Application::new(gtk_receiver, pw_sender.clone());

    app.run();

    pw_sender
        .send(GtkMessage::Terminate)
        .expect("Failed to send message");

    pw_thread.join().expect("Pipewire thread panicked");

    Ok(())
}
