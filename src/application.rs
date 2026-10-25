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

use adw::{
    gio,
    glib::{self, clone},
    gtk,
    prelude::*,
    subclass::prelude::*,
};
use log::error;
use pipewire::channel::Sender;

use crate::{graph_manager::GraphManager, ui, GtkMessage, PipewireMessage};

static STYLE: &str = include_str!("style.css");
static APP_ID: &str = "io.github.spacedouut.Conduit";
static VERSION: &str = env!("CARGO_PKG_VERSION");
static AUTHORS: &str = env!("CARGO_PKG_AUTHORS");

const DEFAULT_REMOTE_NAME: &str = "Default Remote";

mod imp {
    use super::*;

    use std::cell::{Cell, OnceCell};

    use adw::subclass::prelude::AdwApplicationImpl;

    #[derive(Default)]
    pub struct Application {
        pub(super) window: ui::Window,
        pub(super) graph_manager: OnceCell<GraphManager>,
        /// Set by `--hidden` (used by the autostart entry): the first
        /// activation only starts Conduit in the tray.
        pub(super) start_hidden: Cell<bool>,
        pub(super) window_presented: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Application {
        const NAME: &'static str = "ConduitApplication";
        type Type = super::Application;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for Application {}
    impl ApplicationImpl for Application {
        fn activate(&self) {
            let app = &*self.obj();

            let graphview = self.window.graph();

            self.window.set_application(Some(app));

            let zoom_set_action =
                gio::SimpleAction::new("set-zoom", Some(&f64::static_variant_type()));
            zoom_set_action.connect_activate(clone!(@weak graphview => move|_, param| {
                let zoom_factor = param.unwrap().get::<f64>().unwrap();
                graphview.set_zoom_factor(zoom_factor, None)
            }));
            self.window.add_action(&zoom_set_action);

            // The hidden window still keeps the app alive for the tray.
            if !self.start_hidden.replace(false) {
                self.present_window();
            }
        }

        fn startup(&self) {
            self.parent_startup();

            self.obj()
                .style_manager()
                .set_color_scheme(adw::ColorScheme::PreferDark);

            // Load CSS from the STYLE variable.
            let provider = gtk::CssProvider::new();
            provider.load_from_data(STYLE);
            gtk::style_context_add_provider_for_display(
                &gtk::gdk::Display::default().expect("Error initializing gtk css provider."),
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );

            self.setup_actions();
        }
    }
    impl GtkApplicationImpl for Application {}
    impl AdwApplicationImpl for Application {}

    impl Application {
        fn setup_actions(&self) {
            let obj = &*self.obj();

            // Add <Control-Q> shortcut for quitting the application.
            let quit = gtk::gio::SimpleAction::new("quit", None);
            quit.connect_activate(clone!(@weak obj => move |_, _| {
                obj.quit();
            }));
            obj.set_accels_for_action("app.quit", &["<Control>Q"]);
            obj.add_action(&quit);

            let action_about = gio::ActionEntry::builder("about")
                .activate(|obj: &super::Application, _, _| {
                    obj.imp().show_about_dialog();
                })
                .build();
            obj.add_action_entries([action_about]);

            let autostart = gio::SimpleAction::new_stateful(
                "autostart",
                None,
                &crate::autostart::is_enabled().to_variant(),
            );
            autostart.connect_activate(|action, _| {
                let enable = !action
                    .state()
                    .and_then(|s| s.get::<bool>())
                    .unwrap_or(false);
                action.set_enabled(false);
                let action = action.clone();
                crate::autostart::set_enabled(enable, move |enabled| {
                    action.set_state(&enabled.to_variant());
                    action.set_enabled(true);
                });
            });
            obj.add_action(&autostart);
        }

        pub(super) fn present_window(&self) {
            self.window_presented.set(true);
            self.window.present();
        }

        fn show_about_dialog(&self) {
            let obj = &*self.obj();
            let window = obj.active_window().unwrap();
            let authors: Vec<&str> = AUTHORS.split(':').collect();

            let about_window = adw::AboutWindow::builder()
                .transient_for(&window)
                .application_icon(APP_ID)
                .application_name("Conduit")
                .developer_name("spacedouut")
                .developers(authors)
                .version(VERSION)
                .website("https://github.com/spacedouut/conduit")
                .issue_url("https://github.com/spacedouut/conduit/issues")
                .license_type(gtk::License::Gpl30Only)
                .build();

            about_window.present();
        }

        pub(super) fn setup_options(&self, pw_sender: Sender<GtkMessage>) {
            let obj = &*self.obj();

            obj.add_main_option(
                "socket",
                glib::char::Char::from(b's'),
                glib::OptionFlags::NONE,
                glib::OptionArg::String,
                "PipeWire socket to connect",
                Some("PATH"),
            );

            obj.add_main_option(
                "hidden",
                glib::char::Char::from(0),
                glib::OptionFlags::NONE,
                glib::OptionArg::None,
                "Start in the system tray without opening a window",
                None,
            );

            let current_remote_label = obj.imp().window.current_remote_label();
            obj.connect_handle_local_options(clone!(@strong pw_sender => move |app, opts| {
                app.imp().start_hidden.set(opts.contains("hidden"));
                match opts.lookup::<String>("socket") {
                    Ok(p) => {
                        current_remote_label.set_label(p.as_deref().unwrap_or(DEFAULT_REMOTE_NAME));
                        pw_sender.send(GtkMessage::Connect(p)).unwrap();
                    },
                    Err(e) => error!("Invalid socket path: {e}"),
                }
                -1
            }));
        }
    }
}

glib::wrapper! {
    pub struct Application(ObjectSubclass<imp::Application>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl Application {
    /// Create the view.
    /// This will set up the entire user interface and prepare it for being run.
    pub(super) fn new(
        gtk_receiver: async_channel::Receiver<PipewireMessage>,
        pw_sender: Sender<GtkMessage>,
    ) -> Self {
        let app: Application = glib::Object::builder()
            .property("application-id", APP_ID)
            .build();

        let imp = app.imp();

        imp.setup_options(pw_sender.clone());

        imp.graph_manager
            .set(GraphManager::new(
                &imp.window,
                &imp.window.graph(),
                &imp.window.connection_banner(),
                pw_sender,
                gtk_receiver,
            ))
            .expect("Should be able to set graph manager");

        let (tray_sender, tray_receiver) = async_channel::unbounded();
        crate::tray::spawn(tray_sender);
        glib::MainContext::default().spawn_local(clone!(@strong app => async move {
            while let Ok(action) = tray_receiver.recv().await {
                match action {
                    crate::tray::Action::ShowWindow => app.imp().present_window(),
                    crate::tray::Action::WindowUnreachable => {
                        if app.imp().start_hidden.get() || !app.imp().window_presented.get() {
                            app.imp().start_hidden.set(false);
                            if app.imp().window.application().is_some() {
                                app.imp().present_window();
                            }
                        }
                    }
                    crate::tray::Action::Quit => app.quit(),
                }
            }
        }));

        app
    }
}
