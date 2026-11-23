# Development
This document explains how to build, test and run Conduit while working on it.
For how the code is organized, see [Architecture](architecture.md).

Conduit is built against the GNOME 49 SDK, and CI runs every check inside it. Doing the same
locally means you do not need GTK, libadwaita, PipeWire headers, clang or a Rust toolchain on your
host, only `flatpak` (and `flatpak-builder` for the full Flatpak).
All commands below are run from the root of the repository.

## Setting up the SDK
```shell
$ flatpak remote-add --user --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
$ flatpak install --user flathub org.gnome.Platform//49 org.gnome.Sdk//49 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08 \
    org.freedesktop.Sdk.Extension.llvm20//25.08
```

Cargo needs a few settings to work in the SDK, so define a shell function once per terminal that
runs a command in it:

```shell
sdk() {
  flatpak run --share=network --filesystem="$PWD" \
    --env=PATH=/usr/lib/sdk/rust-stable/bin:/usr/lib/sdk/llvm20/bin:/usr/bin \
    --env=LD_LIBRARY_PATH=/usr/lib/sdk/llvm20/lib \
    --env=BINDGEN_EXTRA_CLANG_ARGS="-include $PWD/build-aux/bindgen-compat.h" \
    --env=CARGO_HOME="$PWD/target/sdk/home" \
    --env=CARGO_TARGET_DIR="$PWD/target/sdk/target" \
    --command="$1" org.gnome.Sdk//49 "${@:2}"
}
```

What the settings are for:

- `PATH` and `LD_LIBRARY_PATH` add the Rust and LLVM SDK extensions. LLVM provides the libclang
  that bindgen uses to generate the PipeWire bindings.
- `BINDGEN_EXTRA_CLANG_ARGS` includes `build-aux/bindgen-compat.h`, which redefines two PipeWire
  macros that newer libclang versions no longer hand to bindgen. The build fails without it.
- `CARGO_HOME` and `CARGO_TARGET_DIR` keep the SDK's downloads and build output under `target/sdk/`
  (which git ignores). Do not point `CARGO_HOME` at your host's `~/.cargo`: if rustup is installed
  there, its `cargo-clippy` shim takes over and `cargo clippy` fails.
- `--share=network` lets cargo download crates.

## Building and testing
```shell
$ sdk cargo build --locked
$ sdk cargo test --locked
$ sdk cargo clippy --locked --all-targets
$ sdk cargo fmt --all -- --check
```

CI (`.github/workflows/ci.yml`) runs the last three on every pull request, and they have to pass.
The first run downloads and compiles all dependencies, which takes a few minutes.

The tests are unit tests next to the code they test. They need neither a display nor a running
PipeWire, which is why most logic that is worth testing lives in plain structs (see "The graph
manager" in the architecture document) instead of in widgets or PipeWire callbacks.

## Running against a live PipeWire graph
The debug binary from `cargo build` has its UI files and stylesheet compiled in, so it can be run
straight from the build directory. It needs your display and the PipeWire socket of your session:

```shell
$ flatpak run --socket=wayland --socket=fallback-x11 --share=ipc --device=dri \
    --filesystem=xdg-run/pipewire-0 --filesystem="$PWD" \
    --command="$PWD/target/sdk/target/debug/conduit" org.gnome.Sdk//49
```

The options before `org.gnome.Sdk//49` are for Flatpak, anything after it is passed to Conduit.

Conduit then shows the graph of the PipeWire instance you are logged in with, and changes you make
are real: links, volumes and virtual devices change on your system. Virtual devices and the links
made in Conduit stay in PipeWire after Conduit quits, until you remove them or PipeWire restarts.

Some things to know:

- **Saved data is thrown away.** Started like this, `~/.config` inside the sandbox is temporary.
  That is handy for trying things out from a clean state. To keep saved data between runs (for
  example to work on restoring links), add `--filesystem=xdg-config/conduit:create`; Conduit then
  reads and writes `~/.config/conduit/` on your host.
- **Log output.** Messages go through GLib's logging. Add `--env=G_MESSAGES_DEBUG=all` to see debug
  messages, including every registry event.
- **Another PipeWire instance.** Pass `--socket=/path/to/socket` to Conduit to connect to a socket
  other than the default, and make that path visible in the sandbox with `--filesystem=`.
  Without a reachable PipeWire, Conduit shows a banner and retries until one appears.
- **Tray icon.** The tray icon needs the session bus and a desktop that shows tray icons. Add
  `--socket=session-bus` to give Conduit the bus. Without a tray, Conduit logs a warning and closing
  the window quits it instead of hiding it.
- **Stopping it.** Close the window, or run `pkill -x conduit`. Killing the `flatpak run` process
  can leave Conduit running.

### When there is no graph to look at
On a machine without a desktop session (a VM, a container), start PipeWire and a session manager
yourself. On Debian or Ubuntu:

```shell
$ sudo apt-get install pipewire pipewire-bin wireplumber
$ export XDG_RUNTIME_DIR=/run/user/$(id -u)
$ systemctl --user start pipewire wireplumber
```

Run the session manager too, not only PipeWire: it is what gives sandboxed clients access to the
graph. Without it, Conduit connects but sees no nodes.

A fresh graph like this only has the dummy drivers. To get nodes with ports to link, run
`pw-loopback` in another terminal, or create a virtual device in Conduit.
`pw-cli ls Node`, `pw-link -l` and `pw-dump` show what PipeWire itself thinks the graph is, which
helps to tell a PipeWire-side problem from a display problem in Conduit.

## Building the Flatpak
To build Conduit the way it is released, with its own sandbox permissions, desktop file and icons:

```shell
$ flatpak-builder --user --force-clean --build-only .flatpak/build build-aux/io.github.spacedouut.Conduit.json
$ flatpak-builder --run .flatpak/build build-aux/io.github.spacedouut.Conduit.json conduit
```

This runs Meson, which builds in release mode and takes longer than `cargo build`. Replace
`--build-only` with `--install` to install the result as `io.github.spacedouut.Conduit`; its saved
data is then in `~/.var/app/io.github.spacedouut.Conduit/config/conduit/`.

## Building without Flatpak
The README lists the dependencies and the Meson commands for a build directly on the host.
`cargo build` also works there once the dependencies are installed.
