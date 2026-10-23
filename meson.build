project(
  'conduit',
  'rust',
  version: '0.6.0',
  license: 'GPL-3.0',
  meson_version: '>=0.59.0'
)

gnome = import('gnome')

base_id = 'io.github.spacedouut.Conduit'

dependency('glib-2.0',     version: '>= 2.66')
dependency('gtk4',         version: '>= 4.4.0')
dependency('libadwaita-1', version: '>= 1.4')
dependency('libpipewire-0.3')

desktop_file_validate = find_program('desktop-file-validate', required: false)
appstream_util = find_program('appstream-util', required: false)
cargo = find_program('cargo', required: true)

# Recent libclang versions do not expose PipeWire's casted ID macros to
# bindgen. Force-include literal equivalents while generating Rust bindings.
bindgen_compat = meson.project_source_root() / 'build-aux' / 'bindgen-compat.h'

prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
datadir = prefix / get_option('datadir')
iconsdir = datadir / 'icons'

meson.add_dist_script(
  'build-aux/dist-vendor.sh',
  meson.project_build_root() / 'meson-dist' / meson.project_name() + '-' + meson.project_version(),
  meson.project_source_root()
)

subdir('src')
subdir('data')

gnome.post_install(
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
