//! The Properties dialog: what a file is, where, how big (folders are measured in the
//! background), when it changed, who owns it, what may be done with it, and which
//! application opens files of its type (changeable there).

use super::model::Item;
use super::util;
use super::window::Window;
use failbrauwser::fs::dirsize::dir_size;
use failbrauwser::fs::format::{human_size, human_time, permissions};
use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn row(grid: &gtk::Grid, y: i32, label: &str, value: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(label));
    l.set_xalign(1.0);
    l.set_yalign(0.0);
    l.style_context().add_class("dim-label");
    let v = gtk::Label::new(Some(value));
    v.set_xalign(0.0);
    v.set_selectable(true);
    v.set_line_wrap(true);
    v.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
    v.set_max_width_chars(48);
    grid.attach(&l, 0, y, 1, 1);
    grid.attach(&v, 1, y, 1, 1);
    v
}

pub fn show(w: &Window) {
    let pane = w.current_pane();
    if pane.location() == Location::Drives {
        super::drives::show_selected_properties(&pane);
        return;
    }
    let items = pane.selected_items();
    let loc = pane.location();
    let now = glib::real_time() / 1_000_000;
    let d = gtk::Dialog::with_buttons(None, Some(&w.win), gtk::DialogFlags::DESTROY_WITH_PARENT, &[("_Close", gtk::ResponseType::Close)]);
    d.set_resizable(false);
    let area = d.content_area();
    area.set_border_width(12);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let grid = gtk::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(6);
    grid.set_margin_top(12);

    let (title, icon) = match items.as_slice() {
        [] => (loc.title(), util::folder_icon()),
        [one] => (one.display_name(), match one {
            Item::Fs(e) => util::icon_for_entry(e),
            Item::Archive(_) => util::icon_for_type(&one.content_type()),
            Item::Trash(_, e) | Item::Found(e, _) => util::icon_for_entry(e),
            Item::FoundInArchive(..) => util::icon_for_type(&one.content_type()),
        }),
        many => (format!("{} items", many.len()), gio::ThemedIcon::new("edit-select-all").upcast()),
    };
    d.set_title(&format!("{title} — Properties"));
    header.pack_start(&gtk::Image::from_gicon(&icon, gtk::IconSize::Dialog), false, false, 0);
    let name = gtk::Label::new(None);
    name.set_markup(&format!("<big><b>{}</b></big>", glib::markup_escape_text(&title)));
    name.set_xalign(0.0);
    name.set_selectable(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    header.pack_start(&name, true, true, 0);

    let y = std::cell::Cell::new(0);
    let add = |label: &str, value: &str| {
        let l = row(&grid, y.get(), label, value);
        y.set(y.get() + 1);
        l
    };
    let add_widget = |label: &str, widget: &gtk::Widget| {
        let l = gtk::Label::new(Some(label));
        l.set_xalign(1.0);
        l.style_context().add_class("dim-label");
        grid.attach(&l, 0, y.get(), 1, 1);
        grid.attach(widget, 1, y.get(), 1, 1);
        y.set(y.get() + 1);
    };
    // A share that stopped answering would hang the dialog: asked only on local disks.
    let local = |p: &std::path::Path| !failbrauwser::ops::gvfs::is_gvfs_path(p);
    let where_ = match &loc {
        Location::Archive(_) => format!("{} (in archive)", loc.display()),
        _ => loc.display(),
    };
    // Paths to measure (local only; archive sizes are known from the listing).
    let mut measure = Vec::new();
    match items.as_slice() {
        [Item::Fs(e) | Item::Found(e, _)] => {
            add("Type:", &util::type_description(&e.content_type));
            if e.kind == failbrauwser::fs::Kind::Symlink {
                let target = std::fs::read_link(&e.path).map(|t| t.display().to_string()).unwrap_or_default();
                add("Link target:", &if e.broken { format!("{target} (broken)") } else { target });
            }
            add("Location:", &where_);
            let size = add("Size:", &if e.is_dir_like() { "Calculating…".into() } else { format!("{} ({} bytes)", human_size(e.size), e.size) });
            if e.is_dir_like() {
                measure.push((e.path.clone(), size));
            }
            add("Modified:", &human_time(e.mtime, now));
            if let Some(m) = Some(&e.path).filter(|p| local(p)).and_then(|p| std::fs::metadata(p).ok()) {
                use std::os::unix::fs::MetadataExt;
                add("Accessed:", &human_time(m.atime(), now));
            }
            add("Permissions:", &format!("{} ({:o})", permissions(e.mode), e.mode & 0o7777));
            add("Owner:", &format!("{} / {}", util::user_name(e.uid), util::group_name(e.gid)));
            if !e.is_dir_like() {
                add_widget("Opens with:", default_app_chooser(&d, &e.content_type).upcast_ref());
            }
            if let Some(free) = Some(e.path.parent().unwrap_or(&e.path)).filter(|p| local(p)).and_then(super::window::free_space) {
                add("Free space:", &human_size(free));
            }
        }
        [Item::Trash(t, e)] => {
            add("Type:", &util::type_description(&e.content_type));
            add("Deleted from:", &t.original.parent().map(|p| p.display().to_string()).unwrap_or_default());
            add("Deleted:", &human_time(t.deleted_unix(), now));
            let size = add("Size:", &if e.is_dir_like() { "Calculating…".into() } else { format!("{} ({} bytes)", human_size(e.size), e.size) });
            if e.is_dir_like() {
                measure.push((e.path.clone(), size));
            }
            add("In the trash as:", &e.path.display().to_string());
        }
        [Item::Archive(n) | Item::FoundInArchive(_, n, _)] => {
            add("Type:", &util::type_description(&items[0].content_type()));
            add("Location:", &where_);
            let size = if n.is_dir {
                let total = match &loc {
                    Location::Archive(a) => failbrauwser::archive::vfs::Vfs::global().cached(a).map(|x| x.tree.total_size(&n.path)).unwrap_or(0),
                    _ => 0,
                };
                human_size(total)
            } else {
                format!("{} ({} compressed)", human_size(n.size), human_size(n.compressed))
            };
            add("Size:", &size);
            if let Some(t) = n.mtime {
                add("Modified:", &human_time(t, now));
            }
            if n.encrypted {
                add("Encrypted:", "yes");
            }
        }
        [] => {
            add("Location:", &where_);
            if let Some(p) = loc.local_path() {
                let size = add("Size:", "Calculating…");
                measure.push((p.to_path_buf(), size));
                if let Some(free) = Some(p).filter(|p| local(p)).and_then(super::window::free_space) {
                    add("Free space:", &human_size(free));
                }
            }
        }
        many => {
            add("Location:", &where_);
            let files: u64 = many.iter().filter(|i| !i.is_dir_like()).map(Item::size).sum();
            let size = add("Size:", &human_size(files));
            let dirs: Vec<_> = many.iter().filter(|i| i.is_dir_like()).filter_map(|i| i.path().cloned()).collect();
            if !dirs.is_empty() {
                size.set_text("Calculating…");
                // Measured together below; the label shows the sum.
                let cancel = Arc::new(AtomicBool::new(false));
                spawn_sum(dirs, files, size, cancel.clone());
                d.connect_destroy(move |_| cancel.store(true, Ordering::Relaxed));
            }
        }
    }
    for (path, label) in measure {
        let cancel = Arc::new(AtomicBool::new(false));
        spawn_sum(vec![path], 0, label, cancel.clone());
        d.connect_destroy(move |_| cancel.store(true, Ordering::Relaxed));
    }
    area.add(&header);
    area.add(&grid);
    d.connect_response(|d, _| d.close());
    d.show_all();
}

/// The applications that open `content_type`, the default one chosen. Choosing another one
/// makes it the default for every file of this type (in `mimeapps.list`, shared with the
/// rest of the desktop).
fn default_app_chooser(dialog: &gtk::Dialog, content_type: &str) -> gtk::AppChooserButton {
    let chooser = gtk::AppChooserButton::new(content_type);
    chooser.set_show_default_item(true);
    chooser.set_show_dialog_item(true);
    chooser.set_heading(&format!("Open {} files with", util::type_description(content_type)));
    chooser.set_tooltip_text(Some("The application that opens all files of this type"));
    chooser.set_widget_name("default-app");
    let ct = content_type.to_string();
    let parent = dialog.clone();
    chooser.connect_changed(move |c| {
        let Some(app) = c.app_info() else { return };
        if gio::AppInfo::default_for_type(&ct, false).is_some_and(|d| d.equal(&app)) {
            return;
        }
        if let Err(e) = app.set_as_default_for_type(&ct) {
            util::show_error(&parent, "Cannot change the default application", &e.to_string());
        }
    });
    chooser
}

/// Measures folders on a worker thread and writes "1.2 GB (3,456 files)" into `label`.
fn spawn_sum(dirs: Vec<std::path::PathBuf>, extra: u64, label: gtk::Label, cancel: Arc<AtomicBool>) {
    let label = glib::SendWeakRef::from(label.downgrade());
    glib::spawn_future_local(async move {
        let c2 = cancel.clone();
        let res = gio::spawn_blocking(move || {
            let mut bytes = extra;
            let mut files = 0;
            let mut incomplete = false;
            for d in &dirs {
                let s = dir_size(d, &c2, |_| {})?;
                bytes += s.bytes;
                files += s.files;
                incomplete |= s.incomplete;
            }
            Some((bytes, files, incomplete))
        })
        .await;
        let Some(label) = label.upgrade() else { return };
        if let Ok(Some((bytes, files, incomplete))) = res {
            let prefix = if incomplete { "at least " } else { "" };
            label.set_text(&format!("{prefix}{} ({bytes} bytes), {files} files", human_size(bytes)));
        }
    });
}
