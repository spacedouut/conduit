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

//! The PipeWire thread.
//!
//! [`thread_main`] keeps a connection to PipeWire alive and reconnects when it
//! drops. Registry events are handled in [`registry`], events from the bound
//! proxies in [`callbacks`], and changes the GTK thread asks for in
//! [`state_updates`]. Nothing here panics on PipeWire misbehaving: problems are
//! logged and the thread carries on.

mod callbacks;
mod registry;
mod state;
mod state_updates;

use std::{cell::RefCell, collections::HashMap, io::Cursor, rc::Rc, time::Duration};

use adw::glib::clone;
use log::{error, info, warn};
use pipewire::{
    channel::Receiver as PwReceiver,
    context::ContextRc,
    core::PW_ID_CORE,
    device::{Device, DeviceListener},
    keys,
    link::{Link, LinkInfoRef, LinkListener},
    main_loop::MainLoopRc,
    node::{Node, NodeListener},
    port::{Port, PortListener},
    properties::{properties, PropertiesBox},
    registry::GlobalObject,
    spa::{
        pod::{deserialize::PodDeserializer, serialize::PodSerializer, Object, Pod, Value},
        utils::dict::DictRef,
        utils::result::SpaResult,
    },
    types::ObjectType,
};

use crate::{GtkMessage, MediaType, PipewireMessage};
use state::State;

/// How long to wait before trying to reach PipeWire again.
const RECONNECT_INTERVAL: Duration = Duration::from_millis(200);

type GtkSender = async_channel::Sender<PipewireMessage>;
type Proxies = Rc<RefCell<HashMap<u32, ProxyItem>>>;
type SharedState = Rc<RefCell<State>>;

/// Tell the GTK thread about something. If it already went away (Conduit is
/// quitting) there is nobody left to tell, so the message is dropped.
fn notify(sender: &GtkSender, message: PipewireMessage) {
    if let Err(error) = sender.send_blocking(message) {
        warn!("GTK thread is gone; dropping {:?}", error.into_inner());
    }
}

fn property_is_true(props: &DictRef, key: &str) -> bool {
    matches!(props.get(key), Some("1" | "true" | "yes"))
}

fn is_meter(props: &DictRef) -> bool {
    [
        "node.name",
        "node.nick",
        "node.description",
        "media.name",
        "application.name",
        "media.role",
        "port.name",
    ]
    .into_iter()
    .filter_map(|key| props.get(key))
    .any(|value| value.to_ascii_lowercase().contains("meter"))
}

/// Get the nicest possible name for the node, using a fallback chain of possible name attributes
fn get_node_name(props: &DictRef) -> &str {
    props
        .get(&keys::NODE_DESCRIPTION)
        .or_else(|| props.get(&keys::NODE_NICK))
        .or_else(|| props.get(&keys::NODE_NAME))
        .unwrap_or_default()
}

/// The device and route device a hardware node gets its volume from.
fn route_device_from_props(props: &DictRef) -> Option<(u32, i32)> {
    props
        .get("device.id")
        .and_then(|value| value.parse::<u32>().ok())
        .zip(
            props
                .get("card.profile.device")
                .and_then(|value| value.parse::<i32>().ok()),
        )
}

fn get_link_media_type(link_info: &LinkInfoRef) -> MediaType {
    link_info
        .format()
        .and_then(|format| pipewire::spa::param::format_utils::parse_format(format).ok())
        .map(|(media_type, _media_subtype)| media_type)
        .unwrap_or(MediaType::Unknown)
}

fn serialize_pod(value: &Value) -> Option<Vec<u8>> {
    PodSerializer::serialize(Cursor::new(Vec::new()), value)
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|error| warn!("Could not serialize pod: {error:?}"))
        .ok()
}

fn deserialize_object(param: Option<&Pod>) -> Option<Object> {
    match PodDeserializer::deserialize_any_from(param?.as_bytes()) {
        Ok((_, Value::Object(object))) => Some(object),
        _ => None,
    }
}

enum ProxyItem {
    Node {
        proxy: Node,
        _listener: NodeListener,
        channels: usize,
        /// Last `channelVolumes` reported by the node, used to keep balance.
        channel_volumes: Vec<f32>,
        /// `(device.id, card.profile.device)` for nodes whose volume lives on
        /// a device route.
        route_device: Option<(u32, i32)>,
    },
    Device {
        proxy: Device,
        _listener: DeviceListener,
        /// Active route index for each `card.profile.device`.
        routes: HashMap<i32, i32>,
    },
    Port {
        proxy: Port,
        _listener: PortListener,
    },
    Link {
        _proxy: Link,
        _listener: LinkListener,
    },
}

struct LoopState {
    is_stopped: bool,
    props: PropertiesBox,
}

impl LoopState {
    fn handle_message(&mut self, msg: GtkMessage) -> bool {
        match msg {
            GtkMessage::Terminate => self.is_stopped = true,
            GtkMessage::Connect(remote) => match remote {
                Some(s) => self.props.insert(*keys::REMOTE_NAME, s),
                None => self.props.remove(*keys::REMOTE_NAME),
            },
            _ => return false,
        }
        true
    }
}

/// Run the main loop until the GTK thread asks to connect or quit, or until
/// `timeout` passes.
fn wait_for_control_message(
    mainloop: &MainLoopRc,
    receiver: PwReceiver<GtkMessage>,
    loop_state: &Rc<RefCell<LoopState>>,
    timeout: Option<Duration>,
) -> PwReceiver<GtkMessage> {
    let timer = timeout.map(|timeout| {
        let timer = mainloop
            .loop_()
            .add_timer(clone!(@strong mainloop => move |_| mainloop.quit()));
        (timer.update_timer(Some(timeout), None).into_result(), timer)
    });
    if let Some((Err(error), _)) = &timer {
        // Without a timer the loop would only wake up on a message; sleep instead.
        warn!("Could not arm the reconnect timer ({error}); sleeping instead");
        std::thread::sleep(timeout.unwrap_or_default());
        return receiver;
    }

    let attached = receiver.attach(mainloop.loop_(), {
        clone!(@strong mainloop, @strong loop_state => move |msg|
            if loop_state.borrow_mut().handle_message(msg) {
                mainloop.quit();
            }
        )
    });
    mainloop.run();
    drop(timer);
    attached.deattach()
}

/// The "main" function of the pipewire thread.
pub(super) fn thread_main(gtk_sender: GtkSender, mut pw_receiver: PwReceiver<GtkMessage>) {
    let mainloop = match MainLoopRc::new(None) {
        Ok(mainloop) => mainloop,
        Err(error) => {
            error!("Could not create the PipeWire main loop: {error}");
            notify(&gtk_sender, PipewireMessage::Connecting);
            return;
        }
    };
    let context = match ContextRc::new(&mainloop, None) {
        Ok(context) => context,
        Err(error) => {
            error!("Could not create the PipeWire context: {error}");
            notify(&gtk_sender, PipewireMessage::Connecting);
            return;
        }
    };
    let loop_state = Rc::new(RefCell::new(LoopState {
        is_stopped: false,
        props: properties! {
            "media.category" => "Manager",
        },
    }));
    let mut is_connecting = false;

    // Wait PipeWire service to connect from command line arguments.
    pw_receiver = wait_for_control_message(&mainloop, pw_receiver, &loop_state, None);

    while !loop_state.borrow().is_stopped {
        // Try to connect
        let props = loop_state.borrow().props.clone();
        let connection = context.connect_rc(Some(props)).and_then(|core| {
            let registry = core.get_registry_rc()?;
            Ok((core, registry))
        });
        let (core, registry) = match connection {
            Ok(connection) => connection,
            Err(error) => {
                if !is_connecting {
                    info!("Could not connect to PipeWire ({error}); retrying");
                    is_connecting = true;
                    notify(&gtk_sender, PipewireMessage::Connecting);
                }
                pw_receiver = wait_for_control_message(
                    &mainloop,
                    pw_receiver,
                    &loop_state,
                    Some(RECONNECT_INTERVAL),
                );
                continue;
            }
        };

        is_connecting = false;
        notify(&gtk_sender, PipewireMessage::Connected);

        // Keep proxies and their listeners alive so that we can receive info events.
        let proxies: Proxies = Rc::new(RefCell::new(HashMap::new()));
        let state: SharedState = Rc::new(RefCell::new(State::new()));
        // Holding these proxies owns the endpoints. They intentionally do not
        // linger, so disconnecting or closing Conduit removes them.
        let virtual_devices: state_updates::VirtualDevices = Rc::new(RefCell::new(HashMap::new()));

        let receiver = pw_receiver.attach(mainloop.loop_(), {
            clone!(@strong mainloop, @strong core, @strong registry, @strong proxies, @strong state, @strong loop_state, @strong virtual_devices, @strong gtk_sender => move |msg| match msg {
                GtkMessage::ToggleLink { port_from, port_to } => state_updates::toggle_link(port_from, port_to, &core, &registry, &state),
                GtkMessage::CreateLink { port_from, port_to } => {
                    if state.borrow().get_link_id(port_from, port_to).is_none() {
                        state_updates::create_link(port_from, port_to, &core, &state);
                    }
                }
                GtkMessage::SetNodeVolume { node_id, volume } => state_updates::set_node_volume(node_id, volume, &proxies),
                GtkMessage::DisconnectNode { node_id } => state_updates::disconnect_node(node_id, &registry, &state),
                GtkMessage::CreateVirtualDevice { id, name, node_name, kind, channels } => state_updates::create_virtual_device(
                    id,
                    name,
                    node_name,
                    kind,
                    channels,
                    &core,
                    &virtual_devices,
                    &gtk_sender,
                ),
                GtkMessage::RemoveVirtualDevice { id } => state_updates::remove_virtual_device(
                    id,
                    &core,
                    &virtual_devices,
                    &gtk_sender,
                ),
                GtkMessage::Terminate | GtkMessage::Connect(_) => {
                    loop_state.borrow_mut().handle_message(msg);
                    mainloop.quit();
                }
            })
        });

        let _listener = core
            .add_listener_local()
            .error(
                clone!(@strong mainloop, @strong gtk_sender => move |id, _seq, res, message| {
                    if id != PW_ID_CORE {
                        warn!("PipeWire error on object {id} ({res}): {message}");
                        return;
                    }

                    if res == -libc::EPIPE {
                        // The connection dropped; leave the loop and reconnect.
                        mainloop.quit();
                    } else {
                        match SpaResult::from_c(res).into_result() {
                            Err(error) => error!("Pipewire Core received error {error}: {message}"),
                            Ok(_) => error!("Pipewire Core received error {res}: {message}"),
                        }
                    }
                }),
            )
            .register();

        let _listener = registry
            .add_listener_local()
            .global(clone!(@strong gtk_sender, @strong registry, @strong proxies, @strong state =>
                move |global: &GlobalObject<&DictRef>| match global.type_ {
                    ObjectType::Node => registry::handle_node(global, &gtk_sender, &registry, &proxies, &state),
                    ObjectType::Port => registry::handle_port(global, &gtk_sender, &registry, &proxies, &state),
                    ObjectType::Link => registry::handle_link(global, &gtk_sender, &registry, &proxies, &state),
                    ObjectType::Device => registry::handle_device(global, &registry, &proxies),
                    _ => {
                        // Other objects are not interesting to us
                    }
                }
            ))
            .global_remove(clone!(@strong gtk_sender, @strong proxies, @strong state => move |id| {
                registry::handle_global_remove(id, &gtk_sender, &proxies, &state);
            }))
            .register();

        mainloop.run();
        pw_receiver = receiver.deattach();

        notify(&gtk_sender, PipewireMessage::Disconnected);
    }
}
