//! Icons in the right-click menus: each item gets the icon of the theme that fits its text,
//! from freedesktop.org's standard names first, then names other themes use for the same.

use gtk::prelude::*;
use gtk::{gio, glib};

/// The menu text without mnemonic underscores and the ellipsis of items that ask first.
pub fn plain_text(label: &str) -> String {
    label.replace('_', "").trim_end_matches('…').trim_end_matches("...").to_string()
}

/// Icon names for a menu item by its text, the preferred first (empty: no icon fits).
pub fn icon_names(label: &str) -> &'static [&'static str] {
    match plain_text(label).as_str() {
        "Open" => &["document-open", "folder-open"],
        "Open in New Tab" => &["tab-new", "window-new", "list-add"],
        "Open With" => &["system-run", "application-x-executable"],
        "Other Application" => &["application-x-executable", "system-run"],
        "Cut" => &["edit-cut"],
        "Copy" => &["edit-copy"],
        "Copy Address" => &["edit-copy", "insert-link"],
        "Paste" | "Paste Into Folder" => &["edit-paste"],
        "Move to Trash" => &["user-trash", "edit-delete"],
        "Delete" | "Delete Permanently" => &["edit-delete", "user-trash-full"],
        "Rename" => &["edit-rename", "document-edit", "accessories-text-editor"],
        "Edit" | "Edit Custom Actions" => &["document-edit", "edit-rename", "preferences-other"],
        "Extract Here" | "Extract To" => &["archive-extract", "extract-archive", "package-x-generic"],
        "Open as Archive" => &["package-x-generic", "application-x-archive", "archive-manager"],
        "Compress" => &["archive-insert", "package-x-generic", "application-x-archive"],
        "Compare" => &["view-split-left-right", "view-dual", "edit-find-replace"],
        "Mount as Drive" | "Mount" => &["media-mount", "drive-harddisk", "drive-removable-media"],
        "Unmount" => &["media-eject", "drive-harddisk"],
        "Eject" => &["media-eject"],
        "Unlock" => &["changes-allow", "dialog-password", "system-lock-screen"],
        "Retry" | "Reload" => &["view-refresh"],
        "Create Link on Desktop" => &["emblem-symbolic-link", "insert-link", "user-desktop"],
        "Create Folder" => &["folder-new"],
        "Create Document" => &["document-new"],
        "Select All" => &["edit-select-all"],
        "Show Hidden Files" | "Show Hidden Entry" => &["view-visible", "view-reveal-symbolic", "view-hidden"],
        "Hide" | "Hide Mounted Volumes" => &["view-hidden", "view-conceal-symbolic"],
        "Properties" => &["document-properties"],
        "Empty Trash" => &["user-trash-full", "edit-clear"],
        "Restore" => &["edit-undo", "document-revert"],
        "Pin to Side Panel" => &["view-pin", "bookmark-new", "list-add"],
        "Add Current Folder" | "Add Server" => &["list-add", "bookmark-new"],
        "Remove" => &["list-remove", "edit-delete"],
        "Move Up" => &["go-up"],
        "Move Down" => &["go-down"],
        "Open as" => &["document-open", "package-x-generic"],
        "Archive" => &["package-x-generic", "application-x-archive", "archive-manager"],
        "VeraCrypt / TrueCrypt" | "BitLocker" | "LUKS" | "dm-crypt (plain)" => &["drive-harddisk-encrypted", "dialog-password", "changes-prevent"],
        "Repository" => &["folder-remote", "network-server"],
        "Clone" => &["folder-download", "folder-remote", "edit-copy"],
        "Commit" => &["vcs-commit", "document-save"],
        "Pull" => &["vcs-pull", "go-down"],
        "Push" => &["vcs-push", "go-up"],
        "Sync" => &["emblem-synchronizing", "view-refresh"],
        "Stash" => &["vcs-stash", "archive-insert", "document-save-as"],
        "Switch/Checkout" => &["vcs-branch", "go-jump"],
        "Revert" => &["document-revert", "edit-undo"],
        "Check for modifications" => &["edit-find", "system-search"],
        "Diff" | "Diff with previous version" => &["vcs-diff", "view-split-left-right", "edit-find-replace"],
        "Show Log" => &["vcs-log", "document-open-recent", "x-office-document"],
        "Browse Repository" => &["folder-remote", "system-file-manager"],
        "Blame" => &["vcs-annotate", "format-justify-left", "accessories-text-editor"],
        "Delete (keep local)" | "Delete and add to ignore list" => &["edit-delete", "list-remove"],
        "Settings" => &["preferences-system", "preferences-other"],
        _ => &[],
    }
}

/// The first of `names` the current icon theme has; else the first of their symbolic
/// forms (Adwaita has only those).
pub fn themed(names: &[&str]) -> Option<String> {
    let theme = gtk::IconTheme::default()?;
    with_symbolic(names).into_iter().find(|n| theme.has_icon(n))
}

/// `names`, then each of them as a symbolic icon.
pub fn with_symbolic(names: &[&str]) -> Vec<String> {
    let symbolic = names.iter().filter(|n| !n.ends_with("-symbolic")).map(|n| format!("{n}-symbolic"));
    names.iter().map(|n| n.to_string()).chain(symbolic).collect()
}

/// The icon for the menu text `label`, when the theme has one.
pub fn icon_for(label: &str) -> Option<gio::ThemedIcon> {
    themed(icon_names(label)).map(|n| gio::ThemedIcon::new(&n))
}

/// An item of a menu model with its action and icon.
pub fn model_item(label: &str, action: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), Some(action));
    if let Some(icon) = icon_for(label) {
        item.set_icon(&icon);
    }
    item
}

/// A submenu of a menu model with its icon.
pub fn model_submenu(label: &str, sub: &gio::Menu) -> gio::MenuItem {
    let item = gio::MenuItem::new_submenu(Some(label), sub);
    if let Some(icon) = icon_for(label) {
        item.set_icon(&icon);
    }
    item
}

/// A menu item showing the icon for `label` (with a mnemonic) in front of it; where the
/// theme has none, an empty space of the same size keeps the texts lined up.
pub fn item(label: &str) -> gtk::MenuItem {
    let item = gtk::MenuItem::new();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let image = match themed(icon_names(label)) {
        Some(name) => gtk::Image::from_icon_name(Some(&name), gtk::IconSize::Menu),
        None => gtk::Image::new(),
    };
    image.set_pixel_size(16);
    image.set_size_request(16, 16);
    let text = gtk::AccelLabel::new("");
    text.set_text_with_mnemonic(label);
    text.set_mnemonic_widget(Some(&item));
    text.set_accel_widget(Some(&item));
    text.set_xalign(0.0);
    row.pack_start(&image, false, false, 0);
    row.pack_start(&text, true, true, 0);
    item.add(&row);
    item
}

/// The text of a menu item, with its mnemonic underscore, also of one made by [`item`]
/// (tests find items by it).
pub fn item_text(item: &gtk::MenuItem) -> Option<glib::GString> {
    item.label().or_else(|| {
        let row = item.child()?.downcast::<gtk::Box>().ok()?;
        row.children().into_iter().find_map(|c| c.downcast::<gtk::Label>().ok()).map(|l| l.label())
    })
}

/// GTK 3 leaves out the icons of items with a submenu in menus made from a model: gives
/// them theirs (`icon` says which by the item's text), in `menu` and its submenus.
pub fn submenu_icons(menu: &gtk::Menu, icon: &dyn Fn(&str) -> Option<gio::Icon>) {
    for m in menu.children().into_iter().filter_map(|c| c.downcast::<gtk::MenuItem>().ok()) {
        let Some(sub) = m.submenu().and_then(|s| s.downcast::<gtk::Menu>().ok()) else { continue };
        let label = if m.find_property("text").is_some() { m.property::<Option<String>>("text") } else { m.label().map(|l| l.to_string()) };
        let text = label.map(|l| l.replace("__", "\u{1}").replace('_', "").replace('\u{1}', "_")).unwrap_or_default();
        if m.find_property("icon").is_some() {
            if let Some(i) = icon(&text) {
                m.set_property("icon", &i);
            }
        }
        submenu_icons(&sub, icon);
    }
}

/// The texts of the items of `menu` (and its submenus) shown without an icon (self-test).
pub fn without_icon(menu: &gtk::Menu) -> Vec<String> {
    let mut out = Vec::new();
    for m in menu.children().into_iter().filter_map(|c| c.downcast::<gtk::MenuItem>().ok()) {
        if m.is::<gtk::SeparatorMenuItem>() {
            continue;
        }
        let text = item_text(&m).map(|t| t.replace('_', "")).unwrap_or_default();
        let image = m.child().and_then(|c| c.downcast::<gtk::Box>().ok()).and_then(|b| b.children().into_iter().find_map(|c| c.downcast::<gtk::Image>().ok()));
        if !image.is_some_and(|i| i.icon_name().is_some()) {
            out.push(text.clone());
        }
        if let Some(sub) = m.submenu().and_then(|s| s.downcast::<gtk::Menu>().ok()) {
            out.extend(without_icon(&sub).into_iter().map(|t| format!("{text}/{t}")));
        }
    }
    out
}

/// The texts of the items of a menu model (and its sections and submenus) without an icon (self-test).
pub fn model_without_icon(m: &gio::MenuModel) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..m.n_items() {
        if let Some(l) = m.item_attribute_value(i, "label", None).and_then(|v| v.get::<String>()) {
            if m.item_attribute_value(i, "icon", None).is_none() {
                out.push(l.replace('_', ""));
            }
        }
        for link in ["section", "submenu"] {
            if let Some(sub) = m.item_link(i, link) {
                out.extend(model_without_icon(&sub));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_text_has_icons() {
        assert_eq!(plain_text("_Properties…"), "Properties");
        assert_eq!(plain_text("Co_mpare"), "Compare");
        assert_eq!(icon_names("Cu_t"), ["edit-cut"]);
        assert_eq!(icon_names("Move to T_rash")[0], "user-trash");
        // Every text of the right-click menus (files, trash, side panel, drives, network).
        for label in [
            "_Open", "Open in New _Tab", "Open _With", "Other _Application…", "Cu_t", "_Copy", "_Copy Address", "_Paste", "_Paste Into Folder",
            "Move to T_rash", "_Delete", "_Delete Permanently", "_Rename…", "_Edit…", "Edit Custom _Actions…", "E_xtract Here", "Extract _To…",
            "Open as _Archive", "C_ompress…", "Co_mpare", "_Mount as Drive", "_Mount", "_Unmount", "_Eject", "_Unlock", "_Retry", "_Reload",
            "Create _Link on Desktop", "Create _Folder…", "Create _Document…", "Select _All", "Show _Hidden Files", "_Show Hidden Entry", "_Hide",
            "_Hide Mounted Volumes", "_Properties…", "_Empty Trash", "_Restore", "_Pin to Side Panel", "_Add Current Folder", "_Add Server…",
            "_Remove", "Re_move", "Move _Up", "Move _Down",
            // Open as and the volume kinds a name cannot tell (M14).
            "Open _as", "_Archive", "VeraCrypt / TrueCrypt", "BitLocker", "LUKS", "dm-crypt (plain)",
            // The Repository submenu (M15).
            "_Repository", "Clone…", "Commit", "Pull", "Push", "Sync", "Stash", "Switch/Checkout…", "Revert",
            "Check for modifications", "Diff", "Diff with previous version", "Show Log", "Browse Repository", "Blame",
            "Rename", "Delete", "Delete (keep local)", "Delete and add to ignore list", "Settings",
        ] {
            assert!(!icon_names(label).is_empty(), "no icon for {label:?}");
        }
        assert!(icon_names("Something Else").is_empty());
    }

    #[test]
    fn symbolic_icons_come_after_the_others() {
        assert_eq!(with_symbolic(&["edit-cut", "view-reveal-symbolic"]), ["edit-cut", "view-reveal-symbolic", "edit-cut-symbolic"]);
    }
}
