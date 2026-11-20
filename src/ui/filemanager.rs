//! "Show in folder" for other applications: the `org.freedesktop.FileManager1` interface that
//! Firefox (its downloads' folder icon), Chromium, Thunderbird, Telegram, VS Code and others
//! call to have the session's file manager open a folder, or a file's folder with the file
//! selected. Its bus name is taken next to the file chooser's (`portal`), and `install.sh`
//! installs a D-Bus service file, so a call also starts failBrauwser when it is not running.

use super::app::AppCtx;
use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::ffi::OsString;
use std::path::PathBuf;
use std::rc::Rc;

pub const BUS_NAME: &str = "org.freedesktop.FileManager1";
const OBJECT: &str = "/org/freedesktop/FileManager1";

const XML: &str = r#"<node>
  <interface name="org.freedesktop.FileManager1">
    <method name="ShowFolders">
      <arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/>
    </method>
    <method name="ShowItems">
      <arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/>
    </method>
    <method name="ShowItemProperties">
      <arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/>
    </method>
  </interface>
</node>"#;

/// What a call asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    Folders,
    Items,
    Properties,
}

/// On the portal thread: the windows are made on the main thread.
pub(super) fn register(conn: &gio::DBusConnection) {
    let Some(iface) = gio::DBusNodeInfo::for_xml(XML).ok().and_then(|n| n.lookup_interface(BUS_NAME)) else { return };
    let _ = conn
        .register_object(OBJECT, &iface)
        .method_call(|_, _, _, _, method, params, invocation| {
            let ask = match method {
                "ShowFolders" => Ask::Folders,
                "ShowItems" => Ask::Items,
                "ShowItemProperties" => Ask::Properties,
                _ => {
                    invocation.return_error(gio::DBusError::UnknownMethod, &format!("no method {method}"));
                    return;
                }
            };
            let uris: Vec<String> = params.child_value(0).get().unwrap_or_default();
            let startup: String = params.child_value(1).get().unwrap_or_default();
            // Answered at once: the caller does not wait for the window.
            invocation.return_value(None);
            glib::MainContext::default().invoke(move || super::portal::with_app(move |app| show(app, ask, &uris, &startup)));
        })
        .build();
}

/// The local path a URI names (a mounted share's too), or `None`.
fn path_of(uri: &str) -> Option<PathBuf> {
    if uri.starts_with('/') {
        return Some(PathBuf::from(uri));
    }
    gio::File::for_uri(uri).path()
}

/// Folders to open, each with the names to select in it, in the order first named.
fn plan(ask: Ask, uris: &[String]) -> (Vec<(PathBuf, Vec<OsString>)>, Vec<String>) {
    let mut folders: Vec<(PathBuf, Vec<OsString>)> = Vec::new();
    let mut remote = Vec::new();
    for uri in uris {
        let Some(path) = path_of(uri) else {
            if super::util::is_remote_uri(uri) {
                remote.push(uri.clone());
            }
            continue;
        };
        let (dir, name) = match (ask, path.parent(), path.file_name()) {
            (Ask::Folders, ..) => (path.clone(), None),
            (_, Some(dir), Some(name)) => (dir.to_path_buf(), Some(name.to_os_string())),
            _ => (path.clone(), None),
        };
        match folders.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, names)) => names.extend(name),
            None => folders.push((dir, name.into_iter().collect())),
        }
    }
    (folders, remote)
}

fn show(app: Option<Rc<AppCtx>>, ask: Ask, uris: &[String], startup: &str) {
    let Some(app) = app else { return };
    let (folders, remote) = plan(ask, uris);
    let mut windows = Vec::new();
    if ask == Ask::Folders {
        // Folders are tabs of one window, like `failbrauwser a b`; network addresses are
        // mounted into tabs of it (the first takes the only tab when no folder was named).
        let w = app.open_window(folders.iter().map(|(d, _)| Location::Dir(d.clone())).collect());
        for (i, uri) in remote.iter().enumerate() {
            super::shares::open_remote(&w, uri, !folders.is_empty() || i > 0);
        }
        windows.push(w);
    } else {
        // A window per folder, its items selected (as other file managers do).
        for (dir, names) in folders {
            let w = app.open_window(vec![Location::Dir(dir)]);
            if !names.is_empty() {
                let pane = w.current_pane();
                pane.select_when_present(names);
                if ask == Ask::Properties {
                    properties_once_selected(&w, 0);
                }
            }
            windows.push(w);
        }
    }
    // The caller's activation token lets the window come to the front (Wayland and X11).
    if !startup.is_empty() {
        for w in &windows {
            w.win.set_startup_id(startup);
        }
    }
}

/// Opens the properties of the window's selection once the folder has been read (a few
/// seconds at most).
fn properties_once_selected(w: &Rc<super::window::Window>, tries: u32) {
    let weak = Rc::downgrade(w);
    glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
        let Some(w) = weak.upgrade() else { return };
        if !w.current_pane().selected_items().is_empty() {
            super::properties::show(&w);
        } else if tries < 100 {
            properties_once_selected(&w, tries + 1);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn items_open_their_folders_once_with_all_their_names() {
        let (f, r) = plan(Ask::Items, &s(&["file:///home/u/Downloads/a%20b.pdf", "file:///home/u/Downloads/c.zip", "file:///tmp/x"]));
        assert_eq!(
            f,
            vec![
                (PathBuf::from("/home/u/Downloads"), vec![OsString::from("a b.pdf"), OsString::from("c.zip")]),
                (PathBuf::from("/tmp"), vec![OsString::from("x")]),
            ]
        );
        assert!(r.is_empty());
    }

    #[test]
    fn folders_open_themselves_and_network_addresses_are_mounted() {
        let (f, r) = plan(Ask::Folders, &s(&["file:///home/u/Downloads", "smb://server/share"]));
        assert_eq!(f, vec![(PathBuf::from("/home/u/Downloads"), vec![])]);
        assert_eq!(r, s(&["smb://server/share"]));
    }
}
