//! Network shares: opening `smb://…` addresses (typed, given on the command line or from
//! the side panel), showing the mounted ones like drives (with their free space), getting a
//! share back that stopped answering, and after the computer wakes up, mounting again what
//! sleep took away.

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
    /// The shares mounted when the computer went to sleep (logind's cycle — Linux only).
    #[cfg(unix)]
    static BEFORE_SLEEP: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// logind's sleep signal (dropped = unsubscribed).
    #[cfg(unix)]
    static SLEEP: RefCell<Option<gio::SignalSubscription>> = const { RefCell::new(None) };
}

/// The network shares GVfs has mounted: (address, local folder).
#[cfg(unix)]
fn mounted_shares() -> Vec<(String, PathBuf, gio::Mount)> {
    gio::VolumeMonitor::get()
        .mounts()
        .into_iter()
        .filter_map(|m| {
            let root = m.root();
            let uri = root.uri().to_string();
            failbrauwser::network::is_share_scheme(&uri).then(|| root.path().map(|p| (uri, p, m.clone()))).flatten()
        })
        .collect()
}

/// The network connections Windows has: mapped drives and credential sessions.
#[cfg(windows)]
fn mounted_shares() -> Vec<failbrauwser::win::wnet::Connection> {
    failbrauwser::win::wnet::connections().unwrap_or_default()
}

/// Mapped network drives are drive letters and already ride the Windows volume list
/// (`win::storage`, with their `share_uri` set for the Disconnect button) — adding them
/// again here would double the rows.
#[cfg(windows)]
pub async fn volumes() -> Vec<(failbrauwser::drives::Volume, Option<failbrauwser::drives::Usage>)> {
    Vec::new()
}

/// The mounted network shares as drives, with their fill level where the server tells it.
#[cfg(unix)]
pub async fn volumes() -> Vec<(failbrauwser::drives::Volume, Option<failbrauwser::drives::Usage>)> {
    let mut out = Vec::new();
    for (uri, path, m) in mounted_shares() {
        let usage = super::phones::fs_usage(&m.root()).await;
        let v = failbrauwser::drives::Volume {
            device: uri.clone(),
            share_uri: uri,
            size: usage.map_or(0, |u| u.total),
            mount_points: vec![path],
            mountable: true,
            ejectable: true,
            rotation_rate: -1,
            ..Default::default()
        };
        out.push((v, usage));
    }
    out
}

/// Disconnects a mapped drive shown as a row; tabs showing it go to the drives page first.
/// The drive letter goes with its profile entry, and the credential session behind the
/// share goes too (harmless when other paths still use it: the redirector drops what is
/// unreferenced on its own).
#[cfg(windows)]
pub fn eject(w: &Rc<Window>, v: &failbrauwser::drives::Volume, leave: impl Fn(&[PathBuf])) {
    leave(&v.mount_points);
    if let Some(point) = v.mount_points.first() {
        if let Err(e) = failbrauwser::win::wnet::disconnect(&point.to_string_lossy(), true) {
            util::show_error(&w.win, &format!("Cannot disconnect “{}”", failbrauwser::network::uri_name(&v.share_uri)), &e.to_string());
        }
    }
}

/// Unmounts a share shown as a drive; tabs showing it go to the drives page first.
#[cfg(unix)]
pub fn eject(w: &Rc<Window>, v: &failbrauwser::drives::Volume, leave: impl Fn(&[PathBuf])) {
    leave(&v.mount_points);
    let Some((_, _, m)) = mounted_shares().into_iter().find(|(u, ..)| *u == v.share_uri) else { return };
    let weak = Rc::downgrade(w);
    let name = failbrauwser::network::uri_name(&v.share_uri);
    glib::spawn_future_local(async move {
        if let Err(e) = m.unmount_with_operation_future(gio::MountUnmountFlags::NONE, gio::MountOperation::NONE).await {
            if let Some(w) = weak.upgrade() {
                util::show_error(&w.win, &format!("Cannot disconnect “{name}”"), &e.to_string());
            }
        }
    });
}

/// Windows has no session-inhibition channel reachable from GTK: the redirector
/// reconnects sessions after sleep on its own, and a share that stayed dead is caught by
/// the per-pane `gvfs::check` and the Reconnect button. Nothing to subscribe to.
#[cfg(windows)]
pub fn follow_sleep(_app: &Rc<super::app::AppCtx>) {}

/// Watches the computer going to sleep and waking up (systemd-logind): what was mounted
/// before is checked afterwards, a share that hangs is reconnected and one that sleep
/// unmounted is mounted again.
#[cfg(unix)]
pub fn follow_sleep(app: &Rc<super::app::AppCtx>) {
    SLEEP.with(|s| {
        if s.borrow().is_some() {
            return;
        }
        let Ok(conn) = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE) else { return };
        let app = Rc::downgrade(app);
        *s.borrow_mut() = Some(conn.subscribe_to_signal(
            Some("org.freedesktop.login1"),
            Some("org.freedesktop.login1.Manager"),
            Some("PrepareForSleep"),
            Some("/org/freedesktop/login1"),
            None,
            gio::DBusSignalFlags::NONE,
            move |sig| {
                let going = sig.parameters.child_value(0).get::<bool>().unwrap_or(false);
                if going {
                    sleeping();
                } else if let Some(app) = app.upgrade() {
                    // The network needs a moment after waking up.
                    let app = Rc::downgrade(&app);
                    glib::timeout_add_local_once(std::time::Duration::from_secs(4), move || {
                        if let Some(app) = app.upgrade() {
                            glib::spawn_future_local(async move { resumed(&app).await });
                        }
                    });
                }
            },
        ));
    });
}

/// Going to sleep (tests): remembers the shares mounted now.
#[cfg(unix)]
pub fn sleeping() {
    let uris: Vec<String> = mounted_shares().into_iter().map(|(u, ..)| u).collect();
    BEFORE_SLEEP.with(|b| *b.borrow_mut() = uris);
}

/// Windows: nothing to remember — sessions survive sleep; a dead one is forgotten below so
/// open panes ask it afresh.
#[cfg(windows)]
pub fn sleeping() {}

/// Windows: forget the cached probe failures so open panes ask their shares again (the
/// redirector itself re-establishes sessions on the next access).
#[cfg(windows)]
pub async fn resumed(_app: &Rc<super::app::AppCtx>) {
    for c in mounted_shares() {
        gvfs::forget(&PathBuf::from(&c.remote));
    }
}

/// After waking up: reconnects shares that hang, mounts again those sleep took away, and
/// reads the tabs showing them again.
#[cfg(unix)]
pub async fn resumed(app: &Rc<super::app::AppCtx>) {
    let before = BEFORE_SLEEP.with(|b| std::mem::take(&mut *b.borrow_mut()));
    let mut now = Vec::new();
    for (uri, path, _) in mounted_shares() {
        let p = path.clone();
        let answers = gio::spawn_blocking(move || gvfs::check(&p, std::time::Duration::from_secs(5)).is_ok()).await.unwrap_or(false);
        now.push((uri, answers));
    }
    let Some(w) = app.windows().into_iter().next() else { return };
    for step in failbrauwser::network::after_resume(&before, &now) {
        let (uri, hung) = match step {
            failbrauwser::network::AfterResume::Reconnect(u) => (u, true),
            failbrauwser::network::AfterResume::Mount(u) => (u, false),
        };
        if hung {
            if let Ok(m) = gio::File::for_uri(&uri).find_enclosing_mount(gio::Cancellable::NONE) {
                let _ = m.unmount_with_operation_future(gio::MountUnmountFlags::FORCE, gio::MountOperation::NONE).await;
            }
            if let Some(root) = gio::File::for_uri(&uri).path() {
                gvfs::forget(&root);
            }
        }
        let app2 = Rc::downgrade(app);
        util::mount_remote(&w.win, &uri, move |res| {
            let (Some(app), Ok(path)) = (app2.upgrade(), res) else { return };
            for w in app.windows() {
                for p in w.panes() {
                    if p.location().local_path().is_some_and(|l| l.starts_with(&path)) {
                        p.reload();
                    }
                }
            }
        });
    }
}

/// Mounts a network address and opens it, in the current tab or a new one.
pub fn open_remote(w: &Rc<Window>, uri: &str, new_tab: bool) {
    let weak = Rc::downgrade(w);
    let text = uri.trim().to_string();
    // SFTP and FTP on Windows run through the app's own stack: the address becomes a
    // location directly — no mount, and no desktop involvement.
    #[cfg(windows)]
    if let Some((target, inner)) = failbrauwser::location::Target::parse_uri(&text) {
        super::remote::remember(&text);
        let loc = failbrauwser::location::Location::Remote(failbrauwser::location::RemoteLoc { target, inner });
        if new_tab {
            w.add_tab(loc, true);
        } else {
            w.navigate(loc);
        }
        return;
    }
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

/// The address a UNC share sits at, in the `smb://` spelling the panel and history use.
#[cfg(windows)]
fn share_uri(root: &Path) -> Option<String> {
    failbrauwser::win::wnet::smb_for_unc(root)
}

/// The address a share was mounted from: GIO knows it while the mount exists; an SMB share's
/// folder name also spells it out.
#[cfg(unix)]
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
    #[cfg(unix)]
    d.set_secondary_text(Some("The server or the GVfs service in between stopped answering. Reconnecting mounts the share again."));
    #[cfg(windows)]
    d.set_secondary_text(Some("The server stopped answering. Reconnecting drops its session and enters the share again."));
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

/// Drops the session to the share (over WNet, never through the hung path), signs in again
/// and re-asks: once it fails twice, the server — not any layer in between — is the answer.
#[cfg(windows)]
fn reconnect(w: &Rc<Window>, root: &Path, restarted: bool) {
    let Some(uri) = share_uri(root) else {
        util::show_error(&w.win, "Cannot reconnect", &format!("The address of “{}” is not known.", gvfs::share_name(root)));
        return;
    };
    let Some(unc) = failbrauwser::win::wnet::unc_for_smb(&uri) else {
        util::show_error(&w.win, "Cannot reconnect", &format!("“{uri}” is not a share path Windows can re-enter."));
        return;
    };
    let _ = failbrauwser::win::wnet::disconnect(&unc, false);
    gvfs::forget(root);
    let weak = Rc::downgrade(w);
    util::mount_remote(&w.win, &uri, move |res| {
        let Some(w) = weak.upgrade() else { return };
        let path = match res {
            Ok(p) => p,
            Err(msg) => {
                util::show_error(&w.win, &format!("Cannot reconnect “{}”", gvfs::share_name(Path::new(&unc))), &msg);
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
                reconnect(&w, &path, true);
            }
        });
    });
}

/// Unmounts the share (forcibly) and mounts it again; when it still does not answer, offers
/// to restart GVfs, which also gets a hung gvfsd-fuse going again.
#[cfg(unix)]
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

/// The share came back but its folder still hangs: gvfsd-fuse itself is stuck. Windows
/// has no such layer between the redirector and the server, so there is nothing to restart.
#[cfg(unix)]
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
    #[cfg(unix)] // the fuse folder naming is what this decodes
    fn smb_share_addresses_from_folder_names() {
        let root = gvfs::fuse_root().join("smb-share:server=192.168.1.198,share=gamingcrypt-drive");
        assert_eq!(share_uri(&root).as_deref(), Some("smb://192.168.1.198/gamingcrypt-drive"));
        assert_eq!(share_uri(&gvfs::fuse_root().join("sftp:host=x")), None);
    }

    #[test]
    #[cfg(windows)] // the UNC path itself spells the address
    fn smb_share_addresses_from_unc_paths() {
        assert_eq!(share_uri(Path::new(r"\\192.168.1.198\GamingCrypt-Drive")).as_deref(), Some("smb://192.168.1.198/GamingCrypt-Drive"));
        assert_eq!(share_uri(Path::new(r"C:\work")), None);
    }
}
