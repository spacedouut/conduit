// Copyright 2026 spacedouut
//
// SPDX-License-Identifier: GPL-3.0-only

use async_channel::Sender;
use ksni::{
    blocking::TrayMethods,
    menu::{MenuItem, StandardItem},
};

#[derive(Clone, Copy)]
pub enum Action {
    ShowWindow,
    Quit,
}

struct ConduitTray {
    action_sender: Sender<Action>,
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

    fn watcher_offline(&self, _: ksni::OfflineReason) -> bool {
        true
    }
}

pub fn spawn(action_sender: Sender<Action>) {
    std::thread::Builder::new()
        .name("conduit-tray".into())
        .spawn(move || {
            let tray = ConduitTray { action_sender };
            match tray.assume_sni_available(true).spawn() {
                Ok(_handle) => loop {
                    std::thread::park();
                },
                Err(error) => log::warn!("Could not start system tray icon: {error}"),
            }
        })
        .expect("Could not start system tray thread");
}
