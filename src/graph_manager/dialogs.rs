// SPDX-License-Identifier: GPL-3.0-only

use adw::{glib, gtk, prelude::*, subclass::prelude::*};

use super::GraphManager;
use crate::{VirtualDeviceChannels, VirtualDeviceKind};

impl GraphManager {
    pub(super) fn show_create_virtual_device_dialog(&self) {
        let dialog = gtk::Dialog::with_buttons(
            Some("Create virtual device"),
            Some(&self.window()),
            gtk::DialogFlags::MODAL,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Create", gtk::ResponseType::Accept),
            ],
        );
        dialog.set_default_response(gtk::ResponseType::Accept);
        dialog.set_response_sensitive(gtk::ResponseType::Accept, false);

        let form = gtk::Grid::builder()
            .row_spacing(12)
            .column_spacing(18)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();

        let introduction = gtk::Label::new(Some(
            "Create a routable endpoint. It keeps working after Conduit quits, and Conduit sets it up again, with its volume and connections, every time it starts.",
        ));
        introduction.set_xalign(0.0);
        introduction.set_wrap(true);
        introduction.add_css_class("virtual-device-introduction");
        form.attach(&introduction, 0, 0, 2, 1);

        let name_label = gtk::Label::new(Some("Name"));
        name_label.set_xalign(1.0);
        let name = gtk::Entry::builder()
            .placeholder_text("Podcast mix")
            .activates_default(true)
            .hexpand(true)
            .build();
        form.attach(&name_label, 0, 1, 1, 1);
        form.attach(&name, 1, 1, 1, 1);

        let kind_label = gtk::Label::new(Some("Device type"));
        kind_label.set_xalign(1.0);
        let kind = gtk::ComboBoxText::new();
        kind.append(Some("sink"), "Virtual output");
        kind.append(Some("source"), "Virtual input");
        kind.set_active_id(Some("sink"));
        form.attach(&kind_label, 0, 2, 1, 1);
        form.attach(&kind, 1, 2, 1, 1);

        let channels_label = gtk::Label::new(Some("Channels"));
        channels_label.set_xalign(1.0);
        let channels = gtk::ComboBoxText::new();
        channels.append(Some("stereo"), "Stereo");
        channels.append(Some("mono"), "Mono");
        channels.set_active_id(Some("stereo"));
        form.attach(&channels_label, 0, 3, 1, 1);
        form.attach(&channels, 1, 3, 1, 1);

        dialog.content_area().append(&form);
        name.connect_changed(glib::clone!(@weak dialog => move |entry| {
            dialog.set_response_sensitive(
                gtk::ResponseType::Accept,
                !entry.text().trim().is_empty(),
            );
        }));
        dialog.connect_response(glib::clone!(
            @weak self as manager,
            @weak name,
            @weak kind,
            @weak channels
            => move |dialog, response| {
                if response == gtk::ResponseType::Accept {
                    let device_kind = if kind.active_id().as_deref() == Some("source") {
                        VirtualDeviceKind::Source
                    } else {
                        VirtualDeviceKind::Sink
                    };
                    let device_channels = if channels.active_id().as_deref() == Some("mono") {
                        VirtualDeviceChannels::Mono
                    } else {
                        VirtualDeviceChannels::Stereo
                    };
                    manager.request_create_virtual_device(
                        name.text().trim(),
                        device_kind,
                        device_channels,
                        true,
                    );
                }
                dialog.close();
            }
        ));

        dialog.present();
        name.grab_focus();
    }

    pub(super) fn show_manage_virtual_devices_dialog(&self) {
        let dialog = gtk::Dialog::with_buttons(
            Some("Virtual devices"),
            Some(&self.window()),
            gtk::DialogFlags::MODAL,
            &[("Done", gtk::ResponseType::Close)],
        );
        let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        list.set_margin_top(16);
        list.set_margin_bottom(16);
        list.set_margin_start(16);
        list.set_margin_end(16);

        let devices = self.imp().devices.borrow().devices();
        for (id, device) in devices {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("virtual-device-row");
            let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
            labels.set_hexpand(true);

            let name = gtk::Label::new(Some(&device.name));
            name.set_xalign(0.0);
            let kind = gtk::Label::new(Some(match device.kind {
                VirtualDeviceKind::Sink => "Virtual output",
                VirtualDeviceKind::Source => "Virtual input",
            }));
            kind.set_xalign(0.0);
            kind.add_css_class("dim-label");
            labels.append(&name);
            labels.append(&kind);
            row.append(&labels);

            let remove = gtk::Button::with_label("Remove");
            remove.add_css_class("destructive-action");
            remove.connect_clicked(glib::clone!(
                @weak self as manager,
                @weak dialog
                => move |_| {
                    manager.request_remove_virtual_device(id);
                    dialog.close();
                }
            ));
            row.append(&remove);
            list.append(&row);
        }

        dialog.content_area().append(&list);
        dialog.connect_response(|dialog, _| dialog.close());
        dialog.present();
    }

    pub(super) fn show_virtual_device_error(&self, message: &str) {
        let dialog = adw::MessageDialog::builder()
            .transient_for(&self.window())
            .modal(true)
            .heading("Virtual device operation failed")
            .body(message)
            .close_response("close")
            .build();
        dialog.add_response("close", "Close");
        dialog.present();
    }

    /// Tell the user which stored hidden-node patterns are not being applied.
    pub(super) fn show_invalid_node_filter_warning(&self) {
        let body = invalid_patterns_message(self.imp().node_filter.borrow().invalid_patterns());
        let dialog = adw::MessageDialog::builder()
            .transient_for(&self.window())
            .modal(true)
            .heading("Invalid hidden node patterns")
            .body(body)
            .close_response("close")
            .default_response("edit")
            .build();
        dialog.add_response("close", "Close");
        dialog.add_response("edit", "Edit Patterns");
        dialog.connect_response(
            Some("edit"),
            glib::clone!(@weak self as manager => move |_, _| manager.show_node_filter_dialog()),
        );
        dialog.present();
    }

    pub(super) fn show_node_filter_dialog(&self) {
        let dialog = gtk::Dialog::with_buttons(
            Some("Hidden node patterns"),
            Some(&self.window()),
            gtk::DialogFlags::MODAL,
            &[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Save", gtk::ResponseType::Accept),
            ],
        );
        let entry = gtk::TextView::new();
        entry
            .buffer()
            .set_text(&self.imp().node_filter.borrow().patterns().join("\n"));
        entry.set_tooltip_text(Some(
            "One regular expression per line; matched PipeWire node.name values are hidden.",
        ));
        dialog.content_area().append(&entry);

        let error_label = gtk::Label::new(None);
        error_label.set_xalign(0.0);
        error_label.set_wrap(true);
        error_label.set_selectable(true);
        error_label.add_css_class("error");
        {
            let node_filter = self.imp().node_filter.borrow();
            let invalid = node_filter.invalid_patterns();
            error_label.set_text(&invalid_patterns_message(invalid));
            error_label.set_visible(!invalid.is_empty());
        }
        dialog.content_area().append(&error_label);

        dialog.connect_response(glib::clone!(
            @weak self as manager,
            @weak entry,
            @weak error_label
            => move |dialog, response| {
                if response == gtk::ResponseType::Accept {
                    let buffer = entry.buffer();
                    let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                    let result = manager
                        .imp()
                        .node_filter
                        .borrow_mut()
                        .set_patterns(crate::node_filter::parse_patterns(&text));
                    if let Err(invalid) = result {
                        error_label.set_text(&invalid_patterns_message(&invalid));
                        error_label.set_visible(true);
                        return;
                    }
                    manager.imp().refresh_item_visibility();
                }
                dialog.close();
            }
        ));
        dialog.present();
    }
}

fn invalid_patterns_message(invalid: &[crate::node_filter::InvalidPattern]) -> String {
    let mut message = String::from("These patterns are not valid regular expressions:");
    for pattern in invalid {
        message.push_str(&format!("\n\u{2022} {pattern}"));
    }
    message
}
