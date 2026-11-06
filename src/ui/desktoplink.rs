//! "Create Link on Desktop" in the right-click menus: puts a freedesktop.org link entry (see
//! [`failbrauwser::desktoplink`]) for a folder, file, shortcut or drive on the desktop.

use super::model::Item;
use super::util;
use super::window::Window;
use failbrauwser::desktoplink;
use failbrauwser::drives::Volume;
use gtk::prelude::*;
use gtk::gio;
use std::path::Path;

/// What a link is made of: the name under the icon, the address and a themed icon name.
/// Without an address it starts failBrauwser with `args` (pages only it can show).
pub struct Link {
    pub name: String,
    pub url: String,
    pub icon: String,
    pub args: Vec<String>,
}

impl Link {
    /// What the link points at: its address, or failBrauwser plus the page arguments when
    /// only failBrauwser can show it (`desktoplink::place` writes the platform's entry).
    fn target(&self) -> desktoplink::Target {
        if !self.url.is_empty() {
            return desktoplink::Target::Url(self.url.clone());
        }
        let mut command = vec![program()];
        command.extend(self.args.iter().cloned());
        desktoplink::Target::Command(command)
    }
}

/// How to start failBrauwser: by name when that finds this program, else its full path.
fn program() -> String {
    let me = std::env::current_exe().ok();
    let by_name = gtk::glib::find_program_in_path("failbrauwser");
    match (me, by_name) {
        (Some(me), Some(found)) if found.canonicalize().ok() == me.canonicalize().ok() => "failbrauwser".into(),
        (Some(me), _) => me.display().to_string(),
        (None, _) => "failbrauwser".into(),
    }
}

/// A link to the drives page.
pub fn for_drives_page(name: &str) -> Link {
    Link { name: name.into(), url: String::new(), icon: known_icon(&["drive-harddisk", "computer"]), args: vec![failbrauwser::location::DRIVES_URI.into()] }
}

/// A link to the network page or one computer's shares on it (failBrauwser shows them).
pub fn for_network_page(name: &str, loc: &failbrauwser::location::Location) -> Link {
    let icon = if matches!(loc, failbrauwser::location::Location::Network(h) if h.is_empty()) { "network-workgroup" } else { "network-server" };
    Link { name: name.into(), url: String::new(), icon: known_icon(&[icon, "network-workgroup"]), args: vec![loc.display()] }
}

/// A link to a network share (`smb://…`), which the desktop mounts when it is opened.
pub fn for_share(name: &str, uri: &str) -> Link {
    Link { name: name.into(), url: uri.into(), icon: known_icon(&["folder-remote", "network-server"]), args: Vec::new() }
}

/// A link to the trash.
pub fn for_trash(name: &str) -> Link {
    Link { name: name.into(), url: failbrauwser::location::TRASH_URI.into(), icon: "user-trash".into(), args: Vec::new() }
}

/// The first of `names` the icon theme has (the last one otherwise).
fn known_icon(names: &[&str]) -> String {
    let theme = gtk::IconTheme::default();
    let found = names.iter().find(|n| theme.as_ref().is_some_and(|t| t.has_icon(n)));
    found.or(names.last()).map_or_else(|| "folder".into(), |n| n.to_string())
}

fn icon_of(icon: &gio::Icon) -> String {
    match icon.downcast_ref::<gio::ThemedIcon>() {
        Some(t) => {
            let names: Vec<String> = t.names().iter().map(|n| n.to_string()).filter(|n| !n.ends_with("-symbolic")).collect();
            known_icon(&names.iter().map(String::as_str).collect::<Vec<_>>())
        }
        None => "text-x-generic".into(),
    }
}

/// A link to a local folder (with the icon of Music, Documents… for those).
pub fn for_dir(path: &Path, name: Option<&str>) -> Option<Link> {
    let name = name.map(str::to_string).or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| path.display().to_string());
    Some(Link { name, url: desktoplink::file_url(path)?, icon: icon_of(&util::place_icon(path)), args: Vec::new() })
}

/// A link to a file or folder in the file view.
pub fn for_item(item: &Item) -> Option<Link> {
    let path = item.path()?;
    if item.is_dir_like() {
        return for_dir(path, Some(&item.display_name()));
    }
    Some(Link { name: item.display_name(), url: desktoplink::file_url(path)?, icon: icon_of(&util::icon_for_type(&item.content_type())), args: Vec::new() })
}

/// A link to a drive: its mount point, or for a phone that is not open, its address (the
/// desktop opens it when the link is clicked).
pub fn for_volume(v: &Volume) -> Option<Link> {
    let url = match v.mount_point() {
        Some(m) => desktoplink::file_url(m)?,
        None if v.is_device() => v.device.clone(),
        None => return None,
    };
    let name = if v.is_device() && !v.label.is_empty() { format!("{} – {}", v.title(), v.label) } else { v.title() };
    Some(Link { name, url, icon: known_icon(&v.icon_names()), args: Vec::new() })
}

/// A link to a mounted volume that is not a drive (a network share).
pub fn for_mount(m: &gio::Mount) -> Link {
    Link { name: m.name().to_string(), url: m.root().uri().to_string(), icon: icon_of(&m.icon()), args: Vec::new() }
}

/// Puts the links on the desktop; says so in the status bar, or what went wrong.
pub fn create(w: &Window, links: &[Link]) {
    let mut made = Vec::new();
    for l in links {
        match desktoplink::place(&l.name, &l.icon, &l.target()) {
            Ok(_) => made.push(l.name.as_str()),
            Err(e) => {
                util::show_error(&w.win, "Cannot create the link on the desktop", &format!("{}: {e}", desktoplink::desktop_dir().display()));
                return;
            }
        }
    }
    match made.as_slice() {
        [] => {}
        [one] => w.set_status(&format!("Link to “{one}” put on the desktop")),
        many => w.set_status(&format!("{} links put on the desktop", many.len())),
    }
}
