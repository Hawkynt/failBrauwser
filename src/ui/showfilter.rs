//! The left of the status bar: show everything, only files or only folders (one of the
//! three, kept for the next start), and the files of all subfolders in one list.

use super::actions::{radio_action, toggle_action};
use super::window::Window;
use failbrauwser::config::ShowOnly;
use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::gio;
use std::rc::Rc;

pub fn install_actions(w: &Rc<Window>) {
    let app = Rc::downgrade(&w.app);
    let initial = w.app.settings.borrow().show_only;
    w.win.add_action(&radio_action("show-only", initial.as_str(), move |v| {
        let Some(app) = app.upgrade() else { return };
        app.settings.borrow_mut().show_only = ShowOnly::parse(v);
        app.save_settings_soon();
        app.for_each_window(|w| {
            w.sync_action_state("show-only", v.to_variant());
            for p in w.panes() {
                p.refilter();
            }
            w.update_status();
        });
    }));
    let weak = Rc::downgrade(w);
    w.win.add_action(&toggle_action("show-subfolders", false, move |on| {
        let Some(w) = weak.upgrade() else { return };
        let pane = w.current_pane();
        match (on, pane.location()) {
            (true, Location::Dir(d)) => pane.navigate(Location::with_subfolders(d)),
            (false, Location::Search(s)) if s.is_listing() => pane.navigate_exactly(s.base()),
            _ => {}
        }
    }));
    w.connect_location_changed(sync);
    sync(w);
}

/// The subfolder button follows the tab: pressed while it lists subfolders, usable in folders.
fn sync(w: &Window) {
    let loc = w.current_location();
    let listing = loc.is_subfolder_listing();
    w.sync_action_state("show-subfolders", listing.to_variant());
    if let Some(a) = w.win.lookup_action("show-subfolders").and_then(|a| a.downcast::<gio::SimpleAction>().ok()) {
        a.set_enabled(listing || matches!(loc, Location::Dir(_)));
    }
}

fn button(icon: &str, tip: &str, b: &impl IsA<gtk::Button>) {
    let b = b.upcast_ref::<gtk::Button>();
    b.set_image(Some(&gtk::Image::from_icon_name(Some(icon), gtk::IconSize::Menu)));
    b.set_tooltip_text(Some(tip));
    b.set_relief(gtk::ReliefStyle::None);
    b.set_focus_on_click(false);
    b.style_context().add_class("fb-status-toggle");
}

/// The four buttons, at the left end of the status bar.
pub fn attach(statusbar: &gtk::Statusbar) {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    group.style_context().add_class("linked");
    let mut first: Option<gtk::RadioButton> = None;
    for (value, icon, tip) in [
        ("all", "view-app-grid-symbolic", "Show files and folders"),
        ("files", "text-x-generic-symbolic", "Show files only"),
        ("folders", "folder-symbolic", "Show folders only"),
    ] {
        let b = match &first {
            None => gtk::RadioButton::new(),
            Some(f) => gtk::RadioButton::from_widget(f),
        };
        b.set_mode(false);
        button(icon, tip, &b);
        b.set_action_name(Some("win.show-only"));
        b.set_action_target_value(Some(&value.to_variant()));
        b.set_widget_name(&format!("show-{value}"));
        group.pack_start(&b, false, false, 0);
        first.get_or_insert(b);
    }
    let sub = gtk::ToggleButton::new();
    button("format-indent-more-symbolic", "Show the files of all subfolders too, in one list", &sub);
    sub.set_action_name(Some("win.show-subfolders"));
    sub.set_widget_name("show-subfolders");
    bar.pack_start(&group, false, false, 0);
    bar.pack_start(&sub, false, false, 6);
    bar.set_valign(gtk::Align::Center);
    statusbar.pack_start(&bar, false, false, 0);
    statusbar.reorder_child(&bar, 0);
    bar.show_all();
}

/// Which buttons are pressed, by name (tests).
pub fn pressed(w: &Window) -> Vec<String> {
    let mut out = Vec::new();
    for (name, a) in [("all", "show-only"), ("files", "show-only"), ("folders", "show-only")] {
        if w.win.action_state(a).and_then(|v| v.get::<String>()).as_deref() == Some(name) {
            out.push(name.to_string());
        }
    }
    if w.win.action_state("show-subfolders").and_then(|v| v.get::<bool>()) == Some(true) {
        out.push("subfolders".into());
    }
    out
}
