Conduit is a GTK4 patchbay for PipeWire, built for visual routing across audio, video, and MIDI devices. It is based on [Helvum](https://gitlab.freedesktop.org/pipewire/helvum) and inspired by the JACK tool [Catia](https://kx.studio/Applications:Catia).

![Screenshot](docs/screenshot.png)

Conduit can also create session-scoped virtual inputs and outputs for more complex routing setups.

# Installing and updating

Every push to `main` is built by GitHub Actions and published to a signed Flatpak
repository at <https://spacedouut.github.io/conduit/>.

Install Conduit from the repository, then keep it up to date with Flatpak:

```shell
$ flatpak install --user https://spacedouut.github.io/conduit/io.github.spacedouut.Conduit.flatpakref
$ flatpak update --user io.github.spacedouut.Conduit
```

Or add the repository explicitly:

```shell
$ flatpak remote-add --user --if-not-exists conduit https://spacedouut.github.io/conduit/conduit.flatpakrepo
$ flatpak install --user conduit io.github.spacedouut.Conduit
$ flatpak update --user io.github.spacedouut.Conduit
```

# Building

## Via flatpak
If you don't have the flathub repo in your remote-list for flatpak you will need to add that first:
```shell
$ flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
```

Then install the required flatpak platform and SDK, if you dont have them already:
```shell
$ flatpak install org.gnome.{Platform,Sdk}//49 org.freedesktop.Sdk.Extension.rust-stable//25.08 org.freedesktop.Sdk.Extension.llvm20//25.08
```

To compile and install as a flatpak, clone the project, change to the project directory, and run:
```shell
$ flatpak-builder --install flatpak-build/ build-aux/io.github.spacedouut.Conduit.json
```

You can then run the app via
```shell
$ flatpak run io.github.spacedouut.Conduit
```

## Manually
For compilation, you will need:

- Meson
- An up-to-date rust toolchain
- `libclang-3.7` or higher
- `libadwaita-1` and `libpipewire-0.3` development packages and their dependencies

To compile and install, run

```shell
$ meson setup build && cd build
$ meson compile
$ meson install
```

in the repository root.
This will install the compiled project files into `/usr/local`.

# License and credits
Conduit is distributed under the terms of the GPL3 license.
See LICENSE for more information.

Conduit is derived from Helvum by Tom A. Wagner and its contributors. Their copyright notices are retained in the source.

Parts of the build system were taken from the [gtk-rust-template](https://gitlab.gnome.org/World/Rust/gtk-rust-template) project,
which is provided under the terms of the [MIT license](https://gitlab.gnome.org/World/Rust/gtk-rust-template/-/blob/master/LICENSE.md).
