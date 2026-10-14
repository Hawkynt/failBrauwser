//! Small UI helpers: icon and type-name caches, user names, launching files.

use failbrauwser::fs::{FileEntry, Kind};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

thread_local! {
    static ICONS: RefCell<HashMap<String, gio::Icon>> = RefCell::new(HashMap::new());
    static LINK_ICONS: RefCell<HashMap<String, gio::Icon>> = RefCell::new(HashMap::new());
    static DESCRIPTIONS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    static USERS: RefCell<HashMap<u32, String>> = RefCell::new(HashMap::new());
    static GROUPS: RefCell<HashMap<u32, String>> = RefCell::new(HashMap::new());
    static SPECIAL_DIRS: RefCell<Option<HashMap<PathBuf, &'static str>>> = const { RefCell::new(None) };
}

/// Drops the caches; used when the last window closes and the process idles as a daemon.
pub fn clear_caches() {
    ICONS.with(|c| c.borrow_mut().clear());
    LINK_ICONS.with(|c| c.borrow_mut().clear());
    DESCRIPTIONS.with(|c| c.borrow_mut().clear());
    super::icons::clear_cache();
    super::thumbnails::clear_cache();
}

fn special_dir_icon(path: &Path) -> Option<&'static str> {
    SPECIAL_DIRS.with(|s| {
        let mut s = s.borrow_mut();
        let map = s.get_or_insert_with(|| {
            let mut m = HashMap::new();
            m.insert(glib::home_dir(), "user-home");
            let dirs = [
                (glib::UserDirectory::Desktop, "user-desktop"),
                (glib::UserDirectory::Documents, "folder-documents"),
                (glib::UserDirectory::Downloads, "folder-download"),
                (glib::UserDirectory::Music, "folder-music"),
                (glib::UserDirectory::Pictures, "folder-pictures"),
                (glib::UserDirectory::PublicShare, "folder-publicshare"),
                (glib::UserDirectory::Templates, "folder-templates"),
                (glib::UserDirectory::Videos, "folder-videos"),
            ];
            for (d, icon) in dirs {
                if let Some(p) = glib::user_special_dir(d) {
                    if p != glib::home_dir() {
                        m.insert(p, icon);
                    }
                }
            }
            m
        });
        map.get(path).copied()
    })
}

/// The themed icon for a MIME type.
pub fn icon_for_type(content_type: &str) -> gio::Icon {
    ICONS.with(|c| {
        c.borrow_mut()
            .entry(content_type.to_string())
            .or_insert_with(|| gio::content_type_get_icon(content_type))
            .clone()
    })
}

/// The icon for a file: special folders get their own, symlinks get the link emblem.
pub fn icon_for_entry(e: &FileEntry) -> gio::Icon {
    if e.kind == Kind::Dir {
        if let Some(name) = special_dir_icon(&e.path) {
            return gio::ThemedIcon::from_names(&[name, "folder"]).upcast();
        }
    }
    let ct = if e.broken { "inode/symlink" } else { e.content_type.as_str() };
    let base = icon_for_type(ct);
    if e.kind != Kind::Symlink {
        return base;
    }
    LINK_ICONS.with(|c| {
        c.borrow_mut()
            .entry(ct.to_string())
            .or_insert_with(|| {
                let emblem = gio::Emblem::new(&gio::ThemedIcon::new("emblem-symbolic-link"));
                gio::EmblemedIcon::new(&base, Some(&emblem)).upcast()
            })
            .clone()
    })
}

/// A symbolic icon for a folder in the side panel: the special folders (Documents, Music, …)
/// get their own.
pub fn place_icon(path: &Path) -> gio::Icon {
    let name = special_dir_icon(path).unwrap_or("folder");
    gio::ThemedIcon::from_names(&[&format!("{name}-symbolic"), name]).upcast()
}

pub fn folder_icon() -> gio::Icon {
    icon_for_type("inode/directory")
}

/// "PNG image", "Folder", …
pub fn type_description(content_type: &str) -> String {
    DESCRIPTIONS.with(|c| {
        c.borrow_mut()
            .entry(content_type.to_string())
            .or_insert_with(|| gio::content_type_get_description(content_type).to_string())
            .clone()
    })
}

fn lookup_name(id: u32, group: bool) -> String {
    let mut buf = vec![0u8; 4096];
    unsafe {
        if group {
            let mut grp: libc::group = std::mem::zeroed();
            let mut out: *mut libc::group = std::ptr::null_mut();
            if libc::getgrgid_r(id, &mut grp, buf.as_mut_ptr().cast(), buf.len(), &mut out) == 0 && !out.is_null() {
                return std::ffi::CStr::from_ptr(grp.gr_name).to_string_lossy().into_owned();
            }
        } else {
            let mut pwd: libc::passwd = std::mem::zeroed();
            let mut out: *mut libc::passwd = std::ptr::null_mut();
            if libc::getpwuid_r(id, &mut pwd, buf.as_mut_ptr().cast(), buf.len(), &mut out) == 0 && !out.is_null() {
                return std::ffi::CStr::from_ptr(pwd.pw_name).to_string_lossy().into_owned();
            }
        }
    }
    id.to_string()
}

pub fn user_name(uid: u32) -> String {
    USERS.with(|c| c.borrow_mut().entry(uid).or_insert_with(|| lookup_name(uid, false)).clone())
}

pub fn group_name(gid: u32) -> String {
    GROUPS.with(|c| c.borrow_mut().entry(gid).or_insert_with(|| lookup_name(gid, true)).clone())
}

/// Opens a file with its default application, without blocking the UI.
pub fn open_with_default(parent: &gtk::Window, path: &Path) {
    if super::selftest::active() {
        // No real applications in tests: remember what would have been opened.
        super::selftest::record_opened(path);
        return;
    }
    let file = gio::File::for_path(path);
    let ctx = parent.display().app_launch_context();
    let parent = parent.clone();
    let uri = file.uri();
    gio::AppInfo::launch_default_for_uri_async(&uri, ctx.as_ref(), gio::Cancellable::NONE, move |res| {
        if let Err(e) = res {
            // No default application: let the user pick one, like Thunar does.
            if e.matches(gio::IOErrorEnum::NotSupported) || e.matches(gio::IOErrorEnum::Failed) {
                choose_application(&parent, &[file]);
            } else {
                show_error(&parent, "Could not open file", &e.to_string());
            }
        }
    });
}

/// Launches a specific application with files.
pub fn launch_with(parent: &gtk::Window, app: &gio::AppInfo, files: &[gio::File]) {
    let ctx = parent.display().app_launch_context();
    if let Err(e) = app.launch(files, ctx.as_ref()) {
        show_error(parent, "Could not start application", &e.to_string());
    }
}

/// The native "Open With" chooser.
pub fn choose_application(parent: &gtk::Window, files: &[gio::File]) {
    let Some(first) = files.first() else { return };
    let dialog = gtk::AppChooserDialog::new(Some(parent), gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT, first);
    let files = files.to_vec();
    let parent = parent.clone();
    dialog.connect_response(move |d, resp| {
        if resp == gtk::ResponseType::Ok {
            if let Some(app) = d.app_info() {
                launch_with(&parent, &app, &files);
            }
        }
        d.close();
    });
    dialog.show();
}

pub fn show_error(parent: &impl IsA<gtk::Window>, title: &str, detail: &str) {
    let d = gtk::MessageDialog::new(
        Some(parent),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        gtk::MessageType::Error,
        gtk::ButtonsType::Close,
        title,
    );
    d.set_secondary_text(Some(detail));
    d.connect_response(|d, _| d.close());
    d.show();
}

/// A network address such as `smb://host/share` or `sftp://host/dir`: anything with a
/// scheme the location parser does not handle itself.
pub fn is_remote_uri(text: &str) -> bool {
    let t = text.trim();
    match t.split_once("://") {
        Some((scheme, _)) => {
            !scheme.is_empty()
                && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
                && !matches!(scheme, "file" | "drives" | "trash")
        }
        None => false,
    }
}

/// Mounts a network address through GVfs (asking for a password if the server wants one)
/// and hands back the local folder GVfs exposes it under. Nothing here touches that folder:
/// on a share that stopped answering that would hang the UI (the pane checks it).
pub fn mount_remote(parent: &impl IsA<gtk::Window>, uri: &str, done: impl FnOnce(Result<PathBuf, String>) + 'static) {
    let file = gio::File::for_uri(uri.trim());
    let op = gtk::MountOperation::new(Some(parent));
    let f = file.clone();
    file.mount_enclosing_volume(gio::MountMountFlags::NONE, Some(&op), gio::Cancellable::NONE, move |res| {
        let res = match res {
            Ok(()) => Ok(()),
            Err(e) if e.matches(gio::IOErrorEnum::AlreadyMounted) => Ok(()),
            Err(e) if [gio::IOErrorEnum::NotSupported, gio::IOErrorEnum::NotMountableFile, gio::IOErrorEnum::NotMounted].iter().any(|c| e.matches(*c)) => Err(format!(
                "{e}\n\nThe GVfs backend for this kind of address is not installed \
                 (for smb:// install “gvfs-smb”, for sftp:// “gvfs-sftp”…)."
            )),
            Err(e) => Err(e.to_string()),
        };
        done(res.and_then(|()| f.path().ok_or_else(|| "The share was mounted but has no local folder (is gvfsd-fuse running?).".to_string())));
    });
}

/// Asks for a line of text (new folder name, rename, …). `select_stem` pre-selects the name
/// without its extension, as file managers do for renames.
pub fn ask_text(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    label: &str,
    initial: &str,
    ok_label: &str,
    select_stem: bool,
    on_ok: impl Fn(String) + 'static,
) {
    if super::selftest::active() {
        if let Some(t) = super::selftest::scripted_text() {
            glib::idle_add_local_once(move || on_ok(t));
            return;
        }
    }
    let d = gtk::Dialog::with_buttons(
        Some(title),
        Some(parent),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[("_Cancel", gtk::ResponseType::Cancel), (ok_label, gtk::ResponseType::Ok)],
    );
    d.set_default_response(gtk::ResponseType::Ok);
    d.set_resizable(false);
    let area = d.content_area();
    area.set_spacing(6);
    area.set_border_width(12);
    let l = gtk::Label::new(Some(label));
    l.set_xalign(0.0);
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_activates_default(true);
    entry.set_width_chars(40);
    area.add(&l);
    area.add(&entry);
    d.show_all();
    let stem = if select_stem {
        initial.char_indices().filter(|(i, c)| *c == '.' && *i > 0).map(|(i, _)| i).next().map(|i| initial[..i].chars().count() as i32)
    } else {
        None
    };
    entry.grab_focus();
    entry.select_region(0, stem.unwrap_or(-1));
    d.connect_response(move |d, resp| {
        if resp == gtk::ResponseType::Ok {
            let text = entry.text().to_string();
            if !text.trim().is_empty() {
                on_ok(text);
            }
        }
        d.close();
    });
}

/// Asks for a password (hidden input).
pub fn ask_secret(parent: &impl IsA<gtk::Window>, title: &str, label: &str, on_ok: impl Fn(String) + 'static) {
    if super::selftest::active() {
        if let Some(t) = super::selftest::scripted_text() {
            glib::idle_add_local_once(move || on_ok(t));
            return;
        }
    }
    let d = gtk::Dialog::with_buttons(
        Some(title),
        Some(parent),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[("_Cancel", gtk::ResponseType::Cancel), ("_Unlock", gtk::ResponseType::Ok)],
    );
    d.set_default_response(gtk::ResponseType::Ok);
    d.set_resizable(false);
    let area = d.content_area();
    area.set_spacing(6);
    area.set_border_width(12);
    let l = gtk::Label::new(Some(label));
    l.set_xalign(0.0);
    let entry = gtk::Entry::new();
    entry.set_visibility(false);
    entry.set_input_purpose(gtk::InputPurpose::Password);
    entry.set_activates_default(true);
    entry.set_width_chars(32);
    area.add(&l);
    area.add(&entry);
    d.show_all();
    d.connect_response(move |d, resp| {
        if resp == gtk::ResponseType::Ok {
            on_ok(entry.text().to_string());
        }
        d.close();
    });
}

/// Valid single file name: not empty, no slash, not "." or "..".
pub fn valid_file_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && name != "." && name != ".." && !name.contains('\0')
}

/// Yes/No confirmation.
pub fn confirm(parent: &impl IsA<gtk::Window>, title: &str, detail: &str, ok_label: &str, on_ok: impl Fn() + 'static) {
    if super::selftest::active() {
        glib::idle_add_local_once(on_ok);
        return;
    }
    let d = gtk::MessageDialog::new(
        Some(parent),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        gtk::MessageType::Question,
        gtk::ButtonsType::None,
        title,
    );
    d.set_secondary_text(Some(detail));
    d.add_button("_Cancel", gtk::ResponseType::Cancel);
    d.add_button(ok_label, gtk::ResponseType::Accept);
    d.set_default_response(gtk::ResponseType::Accept);
    d.connect_response(move |d, resp| {
        if resp == gtk::ResponseType::Accept {
            on_ok();
        }
        d.close();
    });
    d.show();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_uris() {
        assert!(is_remote_uri("smb://192.168.1.198/GamingCrypt-Drive"));
        assert!(is_remote_uri(" sftp://me@host/dir "));
        assert!(!is_remote_uri("file:///tmp"));
        assert!(!is_remote_uri("trash:///"));
        assert!(!is_remote_uri("drives:///"));
        assert!(!is_remote_uri("/home/me/a://b"));
    }
}
