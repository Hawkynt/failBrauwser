//! Custom actions in the right-click menu (Thunar's uca.xml and our actions.xml).

use super::model::Item;
use super::util;
use super::window::Window;
use failbrauwser::actions::{self, CustomAction, Target};
use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::path::PathBuf;

fn all() -> Vec<CustomAction> {
    actions::load_all(&actions::thunar_file(), &actions::own_file())
}

/// What the actions work on: the selected files, or the folder shown when nothing is.
fn targets(w: &Window) -> Vec<Target> {
    let pane = w.current_pane();
    let items = pane.selected_items();
    if items.is_empty() {
        return match pane.location() {
            Location::Dir(p) => vec![Target { path: p, is_dir: true, content_type: "inode/directory".into() }],
            _ => Vec::new(),
        };
    }
    items
        .iter()
        .filter_map(|i| match i {
            Item::Fs(e) | Item::Found(e, _) => Some(Target { path: e.path.clone(), is_dir: e.is_dir_like(), content_type: e.content_type.clone() }),
            _ => None,
        })
        .collect()
}

/// Names of the actions shown for the current selection.
pub fn available(w: &Window) -> Vec<String> {
    let t = targets(w);
    all().into_iter().filter(|a| a.applies_to(&t)).map(|a| a.name).collect()
}

/// The menu section with the actions for the current selection.
pub fn menu_section(w: &Window) -> Option<gio::Menu> {
    let t = targets(w);
    let list: Vec<CustomAction> = all().into_iter().filter(|a| a.applies_to(&t)).collect();
    let s = gio::Menu::new();
    for a in &list {
        let item = gio::MenuItem::new(Some(&a.name), None);
        item.set_action_and_target_value(Some("win.custom-action"), Some(&a.name.to_variant()));
        if !a.icon.is_empty() {
            item.set_icon(&gio::ThemedIcon::new(&a.icon));
        }
        s.append_item(&item);
    }
    s.append(Some("Edit Custom _Actions…"), Some("win.edit-custom-actions"));
    Some(s)
}

pub fn install_actions(w: &Window) {
    let a = gio::SimpleAction::new("custom-action", Some(glib::VariantTy::STRING));
    let weak = std::rc::Rc::downgrade(&w.me());
    a.connect_activate(move |_, v| {
        let (Some(w), Some(name)) = (weak.upgrade(), v.and_then(|v| v.get::<String>())) else { return };
        run(&w, &name);
    });
    w.win.add_action(&a);
    w.add_action("edit-custom-actions", edit);
}

fn run(w: &Window, name: &str) {
    let t = targets(w);
    let Some(action) = all().into_iter().find(|a| a.name == name && a.applies_to(&t)) else { return };
    let cmd = action.expand(&t);
    let cwd: PathBuf = w.current_location().nearest_dir().unwrap_or_else(glib::home_dir);
    // The command line was built (quoted) for the shell named here.
    #[cfg(windows)]
    let (shell, flag) = ("cmd", "/C");
    #[cfg(not(windows))]
    let (shell, flag) = ("sh", "-c");
    let spawned = std::process::Command::new(shell)
        .arg(flag)
        .arg(&cmd)
        .current_dir(&cwd)
        .stdin(std::process::Stdio::null())
        .spawn();
    match spawned {
        // Reaped in the background; the program lives on its own.
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => util::show_error(&w.win, &format!("Cannot run “{name}”"), &e.to_string()),
    }
}

/// Opens our actions file in the text editor (created from a template the first time).
fn edit(w: &Window) {
    let file = actions::own_file();
    if !file.exists() {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&file, actions::TEMPLATE) {
            util::show_error(&w.win, "Cannot create the actions file", &e.to_string());
            return;
        }
    }
    util::open_with_default(w.win.upcast_ref(), &file);
}
