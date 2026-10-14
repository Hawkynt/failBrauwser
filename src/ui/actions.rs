//! Application actions, keyboard accelerators and the menu bar model.

use super::app::AppCtx;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::rc::Rc;

/// Accelerators for `app.*` and `win.*` actions. Keys that an entry needs (Delete,
/// Ctrl+C, …) are safe: the window lets a focused entry handle keys first.
pub const ACCELS: &[(&str, &[&str])] = &[
    ("app.new-window", &["<Control>n"]),
    ("app.quit", &["<Control>q"]),
    ("win.new-tab", &["<Control>t"]),
    ("win.close-tab", &["<Control>w"]),
    ("win.close", &["<Control><Shift>w"]),
    ("win.back", &["<Alt>Left"]),
    ("win.forward", &["<Alt>Right"]),
    ("win.up", &["<Alt>Up"]),
    ("win.home", &["<Alt>Home"]),
    ("win.drives", &["<Alt>d"]),
    ("win.location", &["<Control>l", "F6"]),
    ("win.reload", &["F5", "<Control>r"]),
    ("win.show-hidden", &["<Control>h"]),
    ("win.view-mode::list", &["<Control>1"]),
    ("win.view-mode::icons", &["<Control>2"]),
    ("win.zoom-in", &["<Control>plus", "<Control>equal", "<Control>KP_Add"]),
    ("win.zoom-out", &["<Control>minus", "<Control>KP_Subtract"]),
    ("win.zoom-normal", &["<Control>0", "<Control>KP_0"]),
    ("win.show-sidebar", &["F9"]),
    ("win.select-all", &["<Control>a"]),
    ("win.find", &["<Control>f"]),
    ("win.next-tab", &["<Control>Page_Down"]),
    ("win.prev-tab", &["<Control>Page_Up"]),
    ("win.open", &["<Control>o"]),
    ("win.copy", &["<Control>c"]),
    ("win.cut", &["<Control>x"]),
    ("win.paste", &["<Control>v"]),
    ("win.rename", &["F2"]),
    ("win.trash", &["Delete"]),
    ("win.delete", &["<Shift>Delete"]),
    ("win.new-folder", &["<Control><Shift>n"]),
    ("win.properties", &["<Alt>Return"]),
];

pub fn install_app_actions(ctx: &Rc<AppCtx>) {
    let app = &ctx.app;

    let a = gio::SimpleAction::new("new-window", None);
    let c = Rc::downgrade(ctx);
    a.connect_activate(move |_, _| {
        if let Some(c) = c.upgrade() {
            let loc = c.app.active_window().and_then(|w| {
                c.windows().into_iter().find(|x| x.win.upcast_ref::<gtk::Window>() == &w).map(|x| x.current_location())
            });
            c.open_window(loc.into_iter().collect());
        }
    });
    app.add_action(&a);

    let a = gio::SimpleAction::new("quit", None);
    let c = Rc::downgrade(ctx);
    a.connect_activate(move |_, _| {
        if let Some(c) = c.upgrade() {
            for w in c.windows() {
                w.win.close();
            }
            // Quit means quit, daemon or not.
            c.save_settings_now();
            c.app.quit();
        }
    });
    app.add_action(&a);

    let daemon = ctx.settings.borrow().daemon;
    let a = gio::SimpleAction::new_stateful("daemon", None, &daemon.to_variant());
    let c = Rc::downgrade(ctx);
    a.connect_change_state(move |a, v| {
        let Some(c) = c.upgrade() else { return };
        let on = v.and_then(|v| v.get::<bool>()).unwrap_or(false);
        a.set_state(&on.to_variant());
        c.settings.borrow_mut().daemon = on;
        c.apply_daemon();
        c.save_settings_soon();
    });
    app.add_action(&a);

    let smooth = ctx.settings.borrow().smooth_writes;
    let a = gio::SimpleAction::new_stateful("smooth-writes", None, &smooth.to_variant());
    let c = Rc::downgrade(ctx);
    a.connect_change_state(move |a, v| {
        let Some(c) = c.upgrade() else { return };
        let on = v.and_then(|v| v.get::<bool>()).unwrap_or(true);
        a.set_state(&on.to_variant());
        c.settings.borrow_mut().smooth_writes = on;
        c.save_settings_soon();
    });
    app.add_action(&a);

    let verify = ctx.settings.borrow().verify_copies;
    let a = gio::SimpleAction::new_stateful("verify-copies", None, &verify.to_variant());
    let c = Rc::downgrade(ctx);
    a.connect_change_state(move |a, v| {
        let Some(c) = c.upgrade() else { return };
        let on = v.and_then(|v| v.get::<bool>()).unwrap_or(false);
        a.set_state(&on.to_variant());
        c.settings.borrow_mut().verify_copies = on;
        c.save_settings_soon();
    });
    app.add_action(&a);

    let a = gio::SimpleAction::new("about", None);
    let c = Rc::downgrade(ctx);
    a.connect_activate(move |_, _| {
        let Some(c) = c.upgrade() else { return };
        let d = gtk::AboutDialog::new();
        d.set_program_name("failBrauwser");
        d.set_version(Some(env!("CARGO_PKG_VERSION")));
        d.set_comments(Some("A fast file manager that opens archives like folders."));
        d.set_logo_icon_name(Some(&super::app::icon_name()));
        d.set_license_type(gtk::License::Gpl30);
        if let Some(w) = c.app.active_window() {
            d.set_transient_for(Some(&w));
        }
        d.connect_response(|d, _| d.close());
        d.show();
    });
    app.add_action(&a);

    for (action, keys) in ACCELS {
        app.set_accels_for_action(action, keys);
    }
}

fn item(menu: &gio::Menu, label: &str, action: &str) {
    menu.append(Some(label), Some(action));
}

fn section(parent: &gio::Menu, build: impl FnOnce(&gio::Menu)) {
    let s = gio::Menu::new();
    build(&s);
    parent.append_section(None, &s);
}

pub fn menubar_model() -> gio::Menu {
    let bar = gio::Menu::new();

    let file = gio::Menu::new();
    section(&file, |s| {
        item(s, "New _Window", "app.new-window");
        item(s, "New _Tab", "win.new-tab");
    });
    section(&file, |s| {
        item(s, "Create _Folder…", "win.new-folder");
        item(s, "Create _Document…", "win.new-file");
        item(s, "_Compress…", "win.compress");
    });
    section(&file, |s| {
        item(s, "_Properties…", "win.properties");
    });
    section(&file, |s| {
        item(s, "C_lose Tab", "win.close-tab");
        item(s, "_Close Window", "win.close");
        item(s, "_Quit", "app.quit");
    });
    bar.append_submenu(Some("_File"), &file);

    let edit = gio::Menu::new();
    section(&edit, |s| {
        item(s, "Cu_t", "win.cut");
        item(s, "_Copy", "win.copy");
        item(s, "_Paste", "win.paste");
    });
    section(&edit, |s| {
        item(s, "Move to T_rash", "win.trash");
        item(s, "_Delete", "win.delete");
        item(s, "_Rename…", "win.rename");
    });
    section(&edit, |s| {
        item(s, "Select _All", "win.select-all");
        item(s, "_Find…", "win.find");
    });
    section(&edit, |s| {
        item(s, "Keep Running in _Background", "app.daemon");
        item(s, "_Smooth Writes to Slow Drives", "app.smooth-writes");
        item(s, "_Verify Copies", "app.verify-copies");
    });
    bar.append_submenu(Some("_Edit"), &edit);

    let view = gio::Menu::new();
    section(&view, |s| {
        item(s, "_Reload", "win.reload");
    });
    section(&view, |s| {
        item(s, "Show _Hidden Files", "win.show-hidden");
        item(s, "_Folders Before Files", "win.folders-first");
        item(s, "Show _Thumbnails", "win.show-thumbnails");
        item(s, "Side _Panel", "win.show-sidebar");
    });
    section(&view, |s| {
        s.append(Some("as _Detailed List"), Some("win.view-mode::list"));
        s.append(Some("as _Icons"), Some("win.view-mode::icons"));
    });
    section(&view, |s| {
        item(s, "Zoom _In", "win.zoom-in");
        item(s, "Zoom _Out", "win.zoom-out");
        item(s, "_Normal Size", "win.zoom-normal");
    });
    section(&view, |s| {
        s.append_submenu(Some("_Columns"), &columns_menu());
        let tree = gio::Menu::new();
        tree.append(Some("_Show Folder Tree"), Some("win.show-tree"));
        let roots = gio::Menu::new();
        roots.append(Some("Current Folder"), Some("win.tree-root::current"));
        roots.append(Some("Home Folder"), Some("win.tree-root::home"));
        roots.append(Some("File System (/)"), Some("win.tree-root::root"));
        tree.append_section(Some("Tree Starts At"), &roots);
        s.append_submenu(Some("Folder _Tree"), &tree);
    });
    bar.append_submenu(Some("_View"), &view);

    let go = gio::Menu::new();
    section(&go, |s| {
        item(s, "_Back", "win.back");
        item(s, "_Forward", "win.forward");
        item(s, "Open _Parent", "win.up");
    });
    section(&go, |s| {
        item(s, "_Home", "win.home");
        item(s, "_Drives", "win.drives");
        item(s, "Open _Location…", "win.location");
    });
    bar.append_submenu(Some("_Go"), &go);

    let help = gio::Menu::new();
    item(&help, "_About", "app.about");
    bar.append_submenu(Some("_Help"), &help);
    bar
}

pub fn columns_menu() -> gio::Menu {
    let m = gio::Menu::new();
    for (id, label) in [
        ("size", "Size"),
        ("type", "Type"),
        ("modified", "Date Modified"),
        ("permissions", "Permissions"),
        ("owner", "Owner"),
        ("group", "Group"),
    ] {
        m.append(Some(label), Some(&format!("win.column-{id}")));
    }
    m
}

/// Stateful boolean action helper.
pub fn toggle_action(name: &str, initial: bool, on_change: impl Fn(bool) + 'static) -> gio::SimpleAction {
    let a = gio::SimpleAction::new_stateful(name, None, &initial.to_variant());
    a.connect_change_state(move |a, v| {
        let on = v.and_then(|v| v.get::<bool>()).unwrap_or(false);
        a.set_state(&on.to_variant());
        on_change(on);
    });
    a
}

/// Stateful string (radio) action helper.
pub fn radio_action(name: &str, initial: &str, on_change: impl Fn(&str) + 'static) -> gio::SimpleAction {
    let a = gio::SimpleAction::new_stateful(name, Some(glib::VariantTy::STRING), &initial.to_variant());
    a.connect_change_state(move |a, v| {
        let Some(s) = v.and_then(|v| v.get::<String>()) else { return };
        a.set_state(&s.to_variant());
        on_change(&s);
    });
    a
}
