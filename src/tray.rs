// Copyright 2026 spacedouut
//
// SPDX-License-Identifier: GPL-3.0-only

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use async_channel::Sender;
use ksni::{
    blocking::TrayMethods,
    menu::{MenuItem, StandardItem},
};

#[derive(Clone, Copy)]
pub enum Action {
    ShowWindow,
    /// The tray icon is registered with a tray host.
    Available,
    /// There is no tray host, so a hidden window can't be brought back.
    Unavailable,
    Quit,
}

struct ConduitTray {
    action_sender: Sender<Action>,
    online: Arc<AtomicBool>,
}

impl ksni::Tray for ConduitTray {
    fn id(&self) -> String {
        "conduit".into()
    }

    fn title(&self) -> String {
        "Conduit".into()
    }

    fn icon_name(&self) -> String {
        "io.github.spacedouut.Conduit".into()
    }

    fn activate(&mut self, _: i32, _: i32) {
        let _ = self.action_sender.try_send(Action::ShowWindow);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let show_sender = self.action_sender.clone();
        let quit_sender = self.action_sender.clone();
        vec![
            StandardItem {
                label: "Show Conduit".into(),
                activate: Box::new(move |_| {
                    let _ = show_sender.try_send(Action::ShowWindow);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(move |_| {
                    let _ = quit_sender.try_send(Action::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn watcher_online(&self) {
        self.online.store(true, Ordering::SeqCst);
        let _ = self.action_sender.try_send(Action::Available);
    }

    fn watcher_offline(&self, _: ksni::OfflineReason) -> bool {
        self.online.store(false, Ordering::SeqCst);
        let _ = self.action_sender.try_send(Action::Unavailable);
        true
    }
}

pub fn spawn(action_sender: Sender<Action>) {
    let fallback_sender = action_sender.clone();
    let spawned = std::thread::Builder::new()
        .name("conduit-tray".into())
        .spawn(move || {
            let online = Arc::new(AtomicBool::new(true));
            let tray = ConduitTray {
                action_sender: action_sender.clone(),
                online: online.clone(),
            };
            // Flatpak apps may only own names under their app id, so skip the
            // well-known StatusNotifierItem name there.
            let sandboxed = std::path::Path::new("/.flatpak-info").exists();
            match tray
                .disable_dbus_name(sandboxed)
                .assume_sni_available(true)
                .spawn()
            {
                Ok(_handle) => {
                    // Registration failures during spawn already went to
                    // `watcher_offline`.
                    if online.load(Ordering::SeqCst) {
                        let _ = action_sender.try_send(Action::Available);
                    }
                    loop {
                        std::thread::park();
                    }
                }
                Err(error) => {
                    log::warn!("Could not start system tray icon: {error}");
                    let _ = action_sender.try_send(Action::Unavailable);
                }
            }
        });
    if let Err(error) = spawned {
        log::warn!("Could not start system tray thread: {error}");
        let _ = fallback_sender.try_send(Action::Unavailable);
    }
}
