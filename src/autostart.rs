// Copyright 2026 spacedouut
//
// SPDX-License-Identifier: GPL-3.0-only

//! Run-on-startup support. Native installs get an XDG autostart entry; the
//! Flatpak asks the Background portal, since it can't write to the host's
//! autostart directory.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use adw::{gio, glib, prelude::*};

const APP_ID: &str = "io.github.spacedouut.Conduit";
pub const HIDDEN_FLAG: &str = "--hidden";

fn is_flatpak() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}

fn autostart_entry_path() -> PathBuf {
    glib::user_config_dir()
        .join("autostart")
        .join(format!("{APP_ID}.desktop"))
}

/// The portal can't be queried, so the Flatpak remembers its last answer here.
fn portal_state_path() -> PathBuf {
    glib::user_config_dir().join("conduit").join("autostart")
}

pub fn is_enabled() -> bool {
    if is_flatpak() {
        std::fs::read_to_string(portal_state_path()).is_ok_and(|s| s.trim() == "true")
    } else {
        autostart_entry_path().exists()
    }
}

fn desktop_exec_quote(arg: &str) -> String {
    if arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./+=:,".contains(c))
    {
        arg.to_string()
    } else {
        let escaped = arg
            .replace('\\', "\\\\\\\\")
            .replace('"', "\\\\\"")
            .replace('`', "\\\\`")
            .replace('$', "\\\\$");
        format!("\"{escaped}\"")
    }
}

fn autostart_entry(exe: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Conduit\n\
         Comment=Restore Conduit's virtual devices and connections\n\
         Exec={} {HIDDEN_FLAG}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        desktop_exec_quote(exe)
    )
}

fn set_native(enabled: bool) -> Result<(), String> {
    let path = autostart_entry_path();
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        };
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(&path, autostart_entry(&exe.to_string_lossy())).map_err(|e| e.to_string())
}

async fn request_background(enabled: bool) -> Result<bool, String> {
    let connection = gio::bus_get_future(gio::BusType::Session)
        .await
        .map_err(|error| error.to_string())?;
    let sender = connection
        .unique_name()
        .ok_or("no unique bus name")?
        .trim_start_matches(':')
        .replace('.', "_");
    let token = format!("conduit{}", glib::random_int());
    let handle = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");

    let (response_sender, response_receiver) = async_channel::bounded(1);
    let subscription = connection.signal_subscribe(
        Some("org.freedesktop.portal.Desktop"),
        Some("org.freedesktop.portal.Request"),
        Some("Response"),
        Some(&handle),
        None,
        gio::DBusSignalFlags::NONE,
        move |_, _, _, _, _, params| {
            let _ = response_sender.try_send(params.clone());
        },
    );

    let options = glib::VariantDict::new(None);
    options.insert("handle_token", &token);
    options.insert("reason", "Restore virtual devices and connections at login");
    options.insert("autostart", enabled);
    options.insert(
        "commandline",
        vec!["conduit".to_string(), HIDDEN_FLAG.to_string()],
    );
    let params = glib::Variant::tuple_from_iter(["".to_variant(), options.end()]);

    let result = async {
        connection
            .call_future(
                Some("org.freedesktop.portal.Desktop"),
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.portal.Background",
                "RequestBackground",
                Some(&params),
                None,
                gio::DBusCallFlags::NONE,
                -1,
            )
            .await
            .map_err(|error| error.to_string())?;
        let response = response_receiver
            .recv()
            .await
            .map_err(|error| error.to_string())?;
        let (code, results) = response
            .get::<(u32, glib::VariantDict)>()
            .ok_or("unexpected portal response")?;
        if code != 0 {
            return Err(format!("request was cancelled (response {code})"));
        }
        Ok(results
            .lookup::<bool>("autostart")
            .ok()
            .flatten()
            .unwrap_or(false))
    }
    .await;

    #[allow(deprecated)]
    connection.signal_unsubscribe(subscription);
    result
}

/// Turn run-on-startup on or off; `done` gets whether it ended up enabled.
pub fn set_enabled(enabled: bool, done: impl FnOnce(bool) + 'static) {
    if !is_flatpak() {
        if let Err(error) = set_native(enabled) {
            log::warn!("Could not update autostart entry: {error}");
        }
        done(is_enabled());
        return;
    }

    let done = Rc::new(RefCell::new(Some(done)));
    glib::MainContext::default().spawn_local(async move {
        let enabled = match request_background(enabled).await {
            Ok(autostart) => {
                let path = portal_state_path();
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(error) = std::fs::write(&path, if autostart { "true" } else { "false" })
                {
                    log::warn!("Could not save autostart state: {error}");
                }
                autostart
            }
            Err(error) => {
                log::warn!("Background portal request failed: {error}");
                is_enabled()
            }
        };
        if let Some(done) = done.borrow_mut().take() {
            done(enabled);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_entry_starts_hidden() {
        let entry = autostart_entry("/usr/bin/conduit");
        assert!(entry.contains("\nExec=/usr/bin/conduit --hidden\n"));
    }

    #[test]
    fn exec_paths_with_spaces_are_quoted() {
        assert_eq!(
            desktop_exec_quote("/home/me/my apps/conduit"),
            "\"/home/me/my apps/conduit\""
        );
    }
}
