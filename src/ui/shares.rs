//! Network shares: opening `smb://…` addresses (typed, given on the command line or from
//! the side panel), and getting a share back that stopped answering.

use super::util;
use super::window::Window;
use failbrauwser::location::Location;
use failbrauwser::ops::gvfs;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

thread_local! {
    /// Shares a "not responding" dialog is open for (one dialog per share).
    static ASKING: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// Mounts a network address and opens it, in the current tab or a new one.
pub fn open_remote(w: &Rc<Window>, uri: &str, new_tab: bool) {
    let weak = Rc::downgrade(w);
    let text = uri.trim().to_string();
    util::mount_remote(&w.win, &text.clone(), move |res| {
        let Some(w) = weak.upgrade() else { return };
        match res {
            Ok(path) => {
                super::remote::remember(&text);
                if new_tab {
                    w.add_tab(Location::Dir(path), true);
                } else {
                    w.navigate(Location::Dir(path));
                }
            }
            Err(msg) => util::show_error(&w.win, &format!("Cannot open “{text}”"), &msg),
        }
    });
}

/// The address a share was mounted from: GIO knows it while the mount exists; an SMB share's
/// folder name also spells it out.
fn share_uri(root: &Path) -> Option<String> {
    let uri = gio::File::for_path(root).uri().to_string();
    if !uri.starts_with("file:") {
        return Some(uri);
    }
    let dir = root.file_name()?.to_string_lossy().into_owned();
    let rest = dir.strip_prefix("smb-share:")?;
    let field = |key: &str| rest.split(',').find_map(|kv| kv.strip_prefix(key).map(str::to_string));
    Some(format!("smb://{}/{}", field("server=")?, field("share=")?))
}

/// Tells that a share does not answer and offers to mount it again.
pub fn offer_reconnect(w: &Window, root: &Path) {
    let first = ASKING.with(|a| {
        let mut a = a.borrow_mut();
        if a.iter().any(|r| r == root) {
            return false;
        }
        a.push(root.to_path_buf());
        true
    });
    if !first {
        return;
    }
    let d = gtk::MessageDialog::new(
        Some(&w.win),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        gtk::MessageType::Warning,
        gtk::ButtonsType::None,
        &format!("“{}” is not responding", gvfs::share_name(root)),
    );
    d.set_secondary_text(Some("The server or the GVfs service in between stopped answering. Reconnecting mounts the share again."));
    d.add_button("_Cancel", gtk::ResponseType::Cancel);
    d.add_button("_Reconnect", gtk::ResponseType::Accept);
    d.set_default_response(gtk::ResponseType::Accept);
    let weak = Rc::downgrade(&w.me());
    let root = root.to_path_buf();
    d.connect_response(move |d, resp| {
        d.close();
        ASKING.with(|a| a.borrow_mut().retain(|r| *r != root));
        if let (gtk::ResponseType::Accept, Some(w)) = (resp, weak.upgrade()) {
            reconnect(&w, &root, false);
        }
    });
    d.show();
}

/// Unmounts the share (forcibly) and mounts it again; when it still does not answer, offers
/// to restart GVfs, which also gets a hung gvfsd-fuse going again.
fn reconnect(w: &Rc<Window>, root: &Path, restarted: bool) {
    let Some(uri) = share_uri(root) else {
        util::show_error(&w.win, "Cannot reconnect", &format!("The address of “{}” is not known.", gvfs::share_name(root)));
        return;
    };
    let weak = Rc::downgrade(w);
    let root = root.to_path_buf();
    glib::spawn_future_local(async move {
        let file = gio::File::for_uri(&uri);
        // Asks GVfs over D-Bus (not through the hung folder).
        if let Ok(m) = file.find_enclosing_mount(gio::Cancellable::NONE) {
            let _ = m.unmount_with_operation_future(gio::MountUnmountFlags::FORCE, gio::MountOperation::NONE).await;
        }
        gvfs::forget(&root);
        let Some(w) = weak.upgrade() else { return };
        let weak = Rc::downgrade(&w);
        util::mount_remote(&w.win, &uri, move |res| {
            let Some(w) = weak.upgrade() else { return };
            let path = match res {
                Ok(p) => p,
                Err(msg) => {
                    util::show_error(&w.win, &format!("Cannot reconnect “{}”", gvfs::share_name(&root)), &msg);
                    return;
                }
            };
            let weak = Rc::downgrade(&w);
            glib::spawn_future_local(async move {
                let probe = path.clone();
                let ok = gio::spawn_blocking(move || gvfs::check(&probe, gvfs::TIMEOUT).is_ok()).await.unwrap_or(false);
                let Some(w) = weak.upgrade() else { return };
                if ok {
                    for p in w.panes() {
                        if p.location().local_path().is_some_and(|l| l.starts_with(&path)) {
                            p.reload();
                        }
                    }
                } else if restarted {
                    util::show_error(&w.win, &format!("“{}” is still not responding", gvfs::share_name(&path)), "The server does not answer. Check that it is switched on and reachable.");
                } else {
                    offer_restart(&w, &path);
                }
            });
        });
    });
}

/// The share came back but its folder still hangs: gvfsd-fuse itself is stuck.
fn offer_restart(w: &Rc<Window>, root: &Path) {
    let d = gtk::MessageDialog::new(
        Some(&w.win),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        gtk::MessageType::Warning,
        gtk::ButtonsType::None,
        &format!("“{}” is still not responding", gvfs::share_name(root)),
    );
    d.set_secondary_text(Some(
        "The share is mounted, but the GVfs service that shows it as a folder (gvfsd-fuse) seems stuck. \
         Restarting GVfs gets it going again; other network shares are disconnected and may ask for their password.",
    ));
    d.add_button("_Cancel", gtk::ResponseType::Cancel);
    d.add_button("Restart _GVfs", gtk::ResponseType::Accept);
    let weak = Rc::downgrade(w);
    let root = root.to_path_buf();
    d.connect_response(move |d, resp| {
        d.close();
        let (gtk::ResponseType::Accept, Some(w)) = (resp, weak.upgrade()) else { return };
        let weak = Rc::downgrade(&w);
        let root = root.clone();
        glib::spawn_future_local(async move {
            let ok = gio::spawn_blocking(|| {
                std::process::Command::new("systemctl").args(["--user", "restart", "gvfs-daemon"]).status().is_ok_and(|s| s.success())
            })
            .await
            .unwrap_or(false);
            let Some(w) = weak.upgrade() else { return };
            if !ok {
                util::show_error(&w.win, "Cannot restart GVfs", "Running “systemctl --user restart gvfs-daemon” failed.");
                return;
            }
            // Give gvfsd-fuse a moment to come up again.
            glib::timeout_future(std::time::Duration::from_secs(2)).await;
            reconnect(&w, &root, true);
        });
    });
    d.show();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smb_share_addresses_from_folder_names() {
        let root = gvfs::fuse_root().join("smb-share:server=192.168.1.198,share=gamingcrypt-drive");
        assert_eq!(share_uri(&root).as_deref(), Some("smb://192.168.1.198/gamingcrypt-drive"));
        assert_eq!(share_uri(&gvfs::fuse_root().join("sftp:host=x")), None);
    }
}
