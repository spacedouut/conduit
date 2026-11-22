# Architecture
If you want to understand the high-level architecture of conduit,
this document is the right place.

It provides a birds-eye view of the general architecture, then goes through the two threads and
the messages between them, the graph manager, the view, and the files Conduit saves.
To build, test and run Conduit, see [Development](development.md).

Paths are relative to `src/`.

# Top Level Architecture
Conduit uses an architecture with the components laid out like this:

```
┌──────┐
│ GTK  │
│ View │
└────┬─┘
 Λ   ┆
 │<───── updates view                               ┌───────┐
 │   ┆                                              │ State │
 │   ┆<─ notifies of user input                     └───────┘
 │   ┆      (using signals)                             Λ
 │   ┆                                                  │
 │   ┆                                                  │<─── updates/reads state
 │   V           notifies of remote changes             │
┌┴────────────┐         via messages          ┌─────────┴─────────┐
│ Application │<╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤     Seperate      │
│   Object    ├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌>│  Pipewire Thread  │
└─────────────┘   request changes to remote   └───────────────────┘
                        via messages                    Λ
                                                        ║
                                                        ║
                                                        V
                                            [ Remote Pipewire Server ]
```
The program is split between two cooperating threads.
The GTK thread (displayed on the left side) will sit in a GTK event processing loop, while the pipewire thread (displayed on the right side) will sit in a pipewire event processing loop.

The `Application` object inside the GTK thread communicates with the pipewire thread using two channels,
where each message sent by one thread will trigger the loop of the other thread to invoke a callback
with the received message.

For each change on the remote pipewire server, the pipewire thread updates the state and notifies
the `Application` in the GTK thread if changes are needed.
The `Application` then updates the view to reflect those changes.

Additionally, a user may also make changes using the view.
For each change, the view notifies the `Application` by emitting a matching signal.
The `Application` will then ask the pipewire thread to make those changes on the remote. \
These changes will then be applied to the view like any other remote changes as explained above.

In the code, the "Application object" of the diagram is mostly the `GraphManager`
(`graph_manager/`), which `Application` (`application.rs`) creates and hands both channel ends to.
The "State" is `pipewire_connection::state::State`.

A third, small thread runs the system tray icon (`tray.rs`). It only sends `tray::Action`s
(show the window, quit, tray available or unavailable) to `Application` over its own channel.

# The two threads and their messages
`main()` in `main.rs` creates both channels, spawns the PipeWire thread
(`pipewire_connection::thread_main`), and runs the `Application` on the main thread.
When the application quits, it sends `GtkMessage::Terminate` and joins the PipeWire thread.

Both message enums are defined in `main.rs`. A new kind of request or event is a new variant
there, plus a handler on the receiving side.

| | `GtkMessage` | `PipewireMessage` |
|---|---|---|
| Direction | GTK thread → PipeWire thread | PipeWire thread → GTK thread |
| Channel | `pipewire::channel`, attached to the PipeWire loop | `async_channel` (unbounded) |
| Sent with | `GraphManager::send` | `pipewire_connection::notify` |
| Handled in | the `match` in `thread_main` | `GraphManager::handle_message` |

`GtkMessage` variants:

- `Connect(Option<String>)`: connect to the default remote, or to the socket given with `--socket`.
  `Application` sends it once the command line is parsed.
- `ToggleLink`, `CreateLink`: remove or create the link between two ports. `CreateLink` does nothing if
  the link exists, and is used to restore saved links.
- `SetNodeVolume`: set a node's volume, keeping the channel balance.
- `DisconnectNode`: remove every link of a node.
- `CreateVirtualDevice`, `RemoveVirtualDevice`: see [Virtual devices](#virtual-devices).
- `Terminate`: leave the loop so the thread can finish.

`PipewireMessage` variants:

- `NodeAdded`, `NodeNameChanged`, `NodeVolumeChanged`, `NodeRemoved`
- `PortAdded`, `PortFormatChanged`, `PortRemoved`
- `LinkAdded`, `LinkStateChanged`, `LinkFormatChanged`, `LinkRemoved`
- `VirtualDeviceCreated`, `VirtualDeviceRemoved`, `VirtualDeviceError`
- `Connecting`, `Connected`, `Disconnected`
- `CoreCookie`, `RegistrySynced`: see [Node keys](#node-keys)

Nodes, ports and links are identified by their PipeWire global id (`u32`). Ids are only valid for
one connection, so anything that is saved uses names instead (see [Saved data](#saved-data)).

The `id` in the virtual-device messages is different: it is a request id that
`DeviceManager` hands out on the GTK thread, and has nothing to do with PipeWire's ids. The PipeWire
thread finds the device's node by its `node.name`.

Neither side blocks on the other, and neither panics when the other is gone: a failed send is
logged and the message is dropped.

## The PipeWire thread
All of it lives in `pipewire_connection/`:

| File | What it does |
|---|---|
| `mod.rs` | `thread_main`: the connect/reconnect loop, the core and registry listeners, and the dispatch of `GtkMessage`s. Also the `ProxyItem`s that keep bound proxies and their listeners alive. |
| `registry.rs` | Registry events. Binds a proxy for each new node, port, link and device, and reports removed globals. |
| `callbacks.rs` | Info and param events of the bound proxies: names, volumes, port formats, link state, device routes. |
| `state_updates.rs` | The changes the GTK thread asks for: links, volumes, virtual devices. |
| `state.rs` | `State`: the nodes, ports and links seen so far, by id, with a lookup from a port pair to its link. |

`thread_main` first waits for `Connect`. It then tries to connect, and while that fails it sends
`Connecting` once and retries every 200 ms. Once connected it sends `Connected` and runs the loop
until the connection drops (an `EPIPE` error on the core) or a `Connect` or `Terminate` message
arrives. It then sends `Disconnected`, throws away `State` and all proxies, and starts over unless
it was told to terminate.

A node is announced (`NodeAdded`) as soon as the registry lists it. Ports and links are announced
(`PortAdded`, `LinkAdded`) from their first info event, because that is when their direction and
endpoints are known. Volumes are read from the node's `Props` param; for hardware nodes they are
written through the device's `Route` param, like other mixers do, and through `Props` otherwise.

## Virtual devices
A virtual device is a `support.null-audio-sink` adapter node that Conduit creates with
`object.linger`, so it stays in PipeWire after Conduit quits or disconnects. Its `node.name` is
derived from its name and kind (`virtual_device_store::Definition::node_name`,
`conduit.virtual.<kind>.<slug>`).

Because the node may already be running from an earlier launch, `state_updates.rs` holds
`CreateVirtualDevice` requests until the registry has been listed once (a `core.sync` round trip).
A request whose `node.name` is already in `State` adopts that node instead of creating a second
one. Either way the GTK thread gets `VirtualDeviceCreated`. Removal destroys the node through the
registry by name.

Links are created with `object.linger` too, so they also outlive Conduit.

# The graph manager
`GraphManager` (`graph_manager/mod.rs`) is a GObject that lives on the GTK thread. It receives every
`PipewireMessage`, keeps the widget for each PipeWire id in `items`, and connects the widgets'
signals to `GtkMessage`s. It only coordinates; the bookkeeping is in plain structs that take the
data they need as arguments, which keeps them free of GTK:

| Module | What it does |
|---|---|
| `topology.rs` | `Topology`: what the GTK thread knows about the nodes and ports on screen (`node.name`, key, port name and direction, monitor/meter/stream flags). The other modules use it to turn ids into names and back. |
| `link_manager.rs` | `LinkManager`: tracks live links and keeps the saved links of virtual devices in sync with the store. |
| `volume_manager.rs` | `VolumeManager`: restores saved volumes when nodes appear and remembers volume changes. Also the cubic slider scale. |
| `device_manager.rs` | `DeviceManager`: the live virtual devices and the saved ones (it owns the `virtual_device_store::Store`). Builds and validates create requests. |
| `node_slots.rs` | `NodeSlots`: the saved assignment of keys to live nodes that `Topology` uses. |
| `dialogs.rs` | The dialogs for creating and removing virtual devices and for editing hidden-node patterns. |

A node's "key" throughout these modules is what its layout, volume and saved links are stored
under. See [Node keys](#node-keys).

On `Connected`, the manager asks for every saved virtual device to be created again (which adopts
the ones still running). On `Disconnected`, it clears the graph and all per-connection state.

## Node keys
A node's key is its `node.name` (`NodeAdded::layout_key`; the display name if the node has none),
made unique among the nodes that are there at the same time. `Topology::add_node` assigns it: the
first node gets the name itself, and further nodes with the same name (two streams of one
application, for example) get `name#2`, `name#3` and so on, reusing the lowest free one. A `#` or
`\` in the name itself is escaped with a backslash, so a real name can not collide with a suffix.
Hidden-node patterns are the exception: they match the plain `node.name`.

So that the same stream keeps the same key when Conduit restarts while it is running, the
assignment is saved in `node-slots` (`node_slots.rs`), by the node's `object.serial`
(`NodeAdded::serial`). Serials are only meaningful for one run of PipeWire, so the file starts with
the core's cookie and is ignored when the cookie differs. Two messages frame this:

- `CoreCookie` arrives first on each connection; `Topology::begin_enumeration` loads `node-slots`.
  While the existing nodes are being listed, keys saved for other serials stay reserved.
- `RegistrySynced` arrives when everything that already existed has been listed.
  `Topology::finish_enumeration` drops the reservations of nodes that are gone and moves nodes
  that had to avoid one to the key they would otherwise have had. The manager then applies the
  position and volume saved under the new key.

## Links
Only links that touch a saved virtual device are remembered. For those, `LinkManager` decides:

- A new link is saved, unless one end is an application stream (`media.class` `Stream/…`) and the
  user did not make the link in Conduit. The session manager links streams to the default device on
  its own, and those links should not be restored later.
- A removed link is not forgotten immediately. It is suspended for `LINK_FORGET_DELAY` (1 s). If one
  of its ports disappears in that time, the endpoint went away (an app closed, PipeWire shut down)
  and the link stays saved. Otherwise it was removed on purpose and is deleted from the store.
- Whenever a port appears, saved links whose two ends both exist and are not linked are requested
  with `CreateLink`, once each.

## Volumes
A virtual device's volume is saved with its definition. For any other node, a volume is only
remembered once the user changed it in Conduit; after that, changes made elsewhere are followed
too, so a restart restores the latest value. When a remembered node appears, the first volume it
reports is replaced by the saved one. Writes of `node-volumes` are delayed by 500 ms so that
dragging a slider does not rewrite the file on every step.

## Visibility
`GraphManager::refresh_item_visibility` decides what is shown:

- A node is hidden when a hidden-node pattern matches its `node.name`, or when it is a level meter and
  meters are switched off.
- A port is hidden when its node is hidden, when it is a monitor port and monitor ports are
  switched off, or when it is a meter port and meters are switched off.
- A link is hidden when either of its ports is hidden.

Hidden items stay in `items` and `Topology`, so links to them keep being tracked and saved.

# The view
The widgets are in `ui/`. They know nothing about PipeWire connections or saved data; they show
what the graph manager gives them and emit signals for user input.

| File | What it does |
|---|---|
| `window.rs`, `window.ui` | `Window`: the header bar, menu, connection banner and the `GraphView`. |
| `graph/graph_view.rs` | `GraphView`: the canvas. Positions and draws nodes and links, zooms and scrolls, and handles selection, node dragging and port drag-and-drop. |
| `graph/placement.rs` | Default positions for nodes the user has not placed: columns for outputs, others and inputs. |
| `graph/node.rs`, `node.ui` | `Node`: a node with its ports and volume slider, or a comment. Emits `disconnect-all`, `delete-comment` and `show-volume-sliders-changed`. |
| `graph/port.rs`, `port.ui`, `port_handle.rs` | `Port`: a port and the handle links attach to. Emits `port-toggled` when one port is dropped on another. |
| `graph/link.rs`, `link_curve.rs` | `Link`: the data of a link, and the curve it is drawn as. |
| `graph/zoomentry.rs`, `zoomentry.ui` | `ZoomEntry`: the zoom control in the header bar. |

`application.rs` holds the `Application`: command line options (`--socket`, `--hidden`), the
`quit`, `about` and `autostart` actions, and the tray handling. While a tray host is available,
closing the window only hides it. `autostart.rs` implements "Run on Startup": an XDG autostart
entry for native installs, the Background portal in the Flatpak.

# Saved data
Everything is saved under `$XDG_CONFIG_HOME/conduit/` (`glib::user_config_dir()`), which is
`~/.config/conduit/` for a native install and `~/.var/app/io.github.spacedouut.Conduit/config/conduit/`
for the Flatpak. All of it is read and written on the GTK thread.

| File | Module | Contents |
|---|---|---|
| `virtual-devices` | `virtual_device_store.rs` | One device per line: name, `sink` or `source`, `mono` or `stereo`, and optionally its volume. |
| `virtual-device-links` | `virtual_device_store.rs` | One link per line: output node, output port, input node, input port. |
| `node-layout` | `node_layout.rs` | Node positions: node key, x, y. |
| `node-volumes` | `node_volume_store.rs` | Volumes of other nodes: node key, linear volume. |
| `comments` | `comment_store.rs` | Comment nodes: id, x, y, text. |
| `hidden-node-patterns` | `node_filter.rs` | One regular expression per line, matched against the `node.name`. Plain text. |
| `node-slots` | `graph_manager/node_slots.rs` | Which key each live node holds: the PipeWire core's cookie, then object serial, escaped `node.name`, key. See [Node keys](#node-keys). |
| `show-volume-sliders` | `volume_slider_setting.rs` | `true` or `false`. |
| `autostart` | `autostart.rs` | Flatpak only: the last answer of the Background portal. |

The formats are line based with tab-separated fields. Names, node keys, port names and comment
text are written as hex-encoded UTF-8 so they can not contain a tab or a newline. Lines that do not
parse are skipped when loading. Volumes are linear (1.0 is 100%), not the cubic slider value.

The stores save themselves when they change. `persistence.rs` holds what they share: the config
path, the hex encoding, and `write_atomically`, which writes a new temporary file next to the
target and renames it over, so an interrupted save leaves the previous file intact. If the target
is a symlink, the file it points to is replaced and the link is kept. (`node-slots` and `autostart`
do not go through it.) Comment text and `node-volumes` are written 500 ms after the last change,
and `Application::shutdown` writes whatever is still waiting. A failed
save is logged as a warning and the change is kept in memory only.

Nodes are matched by their key (see [Node keys](#node-keys)) and ports by `port.name`, because
PipeWire ids change on every connection.
