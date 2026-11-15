//! Links in the window (README: Links): the menu a right-button drag opens where it drops,
//! *Pick Link Source* and *Drop As* in the right-click menus, the tree operations as jobs with the
//! progress window, and Smart Move after folders are moved or renamed. What is made and how is
//! [`failbrauwser::links`]; this is where it meets the menus.

use super::clipboard::ClipSource;
use super::fileops;
use super::jobs::Outcome;
use super::model::Item;
use super::window::Window;
use failbrauwser::links::{self, tree, LinkKind, Source};
use failbrauwser::location::Location;
use failbrauwser::ops::copy::Mode;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

thread_local! {
    /// What *Pick Link Source* remembered, for *Drop As* (one pick for the whole program, as
    /// Link Shell Extension keeps it).
    static PICKED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    /// The drop menu open last: its model and actions (the self-test reads and clicks it).
    static OPEN: RefCell<Option<(gio::Menu, gio::SimpleActionGroup, gtk::Menu)>> = const { RefCell::new(None) };
}

/// The tree operations: (id, label).
const TREE_OPS: [(&str, tree::TreeOp); 4] = [
    ("tree-smart-copy", tree::TreeOp::SmartCopy),
    ("tree-smart-mirror", tree::TreeOp::SmartMirror),
    ("tree-hardlink-clone", tree::TreeOp::HardlinkClone),
    ("tree-symlink-clone", tree::TreeOp::SymlinkClone),
];

pub fn options(w: &Window) -> tree::Options {
    let s = w.app.settings.borrow();
    tree::Options { outer: s.outer_links, relative: s.relative_symlinks, backup: s.backup_mode }
}

/// failBrauwser itself as the administrator, for Backup Mode and Volume Mountpoints.
fn admin() -> std::io::Result<links::helper::Elevation> {
    Ok(links::helper::Elevation::admin(&std::env::current_exe()?))
}

/// Whether Backup Mode runs through the administrator helper (Linux); Windows enables its
/// backup privileges in the process itself.
fn through_helper(opts: tree::Options) -> bool {
    opts.backup && cfg!(unix)
}

/// How Smart Move runs: relative links or not, Backup Mode or not.
#[derive(Debug, Clone, Copy)]
pub struct SmartMove {
    pub relative: bool,
    pub backup: bool,
}

impl SmartMove {
    pub fn of(w: &Window) -> SmartMove {
        let s = w.app.settings.borrow();
        SmartMove { relative: s.relative_symlinks, backup: s.backup_mode }
    }
}

fn local_paths(sources: &[ClipSource]) -> Vec<PathBuf> {
    sources
        .iter()
        .filter_map(|s| match s {
            ClipSource::Local(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

fn item(label: &str, action: &str, id: &str) -> gio::MenuItem {
    let it = super::menuicons::model_item(label, &format!("{action}::{id}"));
    // A link kind shows the overlay it makes.
    let overlay = match id {
        "link-symbolic" => Some(links::Overlay::Symbolic),
        "link-hard" => Some(links::Overlay::Hard),
        "link-junction" => Some(links::Overlay::Junction),
        _ => None,
    };
    if id == "link-mountpoint" {
        // The overlays draw links; a drive is a drive.
        it.set_icon(&gio::ThemedIcon::from_names(&["drive-harddisk", "drive-removable-media"]));
    }
    if let Some(o) = overlay {
        let icon = gio::BytesIcon::new(&glib::Bytes::from_owned(super::linkicons::svg(o).into_bytes()));
        it.set_icon(&icon);
    }
    it
}

/// The links that can be made from `paths` in `dir`.
fn link_kinds(paths: &[PathBuf], dir: &Path) -> Vec<LinkKind> {
    let sources: Vec<Source> = paths.iter().map(|p| Source::of(p)).collect();
    links::offered(&sources, links::caps_at(dir), failbrauwser::osprim::device_of(dir))
}

/// The tree operations offered for `paths` dropped into `dir` (folders among them).
fn tree_ops(paths: &[PathBuf], dir: &Path) -> Vec<(&'static str, &'static str)> {
    if !paths.iter().any(|p| p.is_dir() && !p.is_symlink()) {
        return Vec::new();
    }
    let caps = links::caps_at(dir);
    let dev = failbrauwser::osprim::device_of(dir);
    let same_fs = dev.is_some() && paths.iter().all(|p| failbrauwser::osprim::entry_device(p) == dev);
    let mut out = Vec::new();
    for (id, op) in TREE_OPS {
        let ok = match op {
            tree::TreeOp::HardlinkClone => caps.hardlinks && same_fs,
            tree::TreeOp::SymlinkClone => caps.symlinks,
            _ => true,
        };
        if ok {
            out.push((id, op.label()));
        }
        if op == tree::TreeOp::SmartMirror && paths.len() == 1 {
            // One folder at a time (Link Shell Extension hides it for several).
            out.push(("delorean", "DeLorean Copy"));
        }
    }
    out
}

/// The *Create Link here* / *Drop As* items for `paths` into `dir`, on `action` (which takes the
/// item's id as its string parameter).
fn link_sections(paths: &[PathBuf], dir: &Path, action: &str, menu: &gio::Menu, nested: bool) {
    let kinds = link_kinds(paths, dir);
    let ops = tree_ops(paths, dir);
    let make = gio::Menu::new();
    for k in &kinds {
        make.append_item(&item(k.label(), action, &format!("link-{}", k.id())));
    }
    let clone = gio::Menu::new();
    for (id, label) in &ops {
        clone.append_item(&item(label, action, id));
    }
    // Links picked or dragged themselves: copied so they reach the same target from there.
    if !paths.is_empty() && paths.iter().all(|p| p.is_symlink() || links::make::is_junction(p)) {
        let label = if paths.iter().all(|p| links::make::is_junction(p)) { "Junction Copy" } else { "Symbolic Link Copy" };
        make.append_item(&item(label, action, "link-copy"));
    }
    if nested {
        let s = gio::Menu::new();
        if !kinds.is_empty() {
            s.append_item(&super::menuicons::model_submenu("Create _Link here", &make));
        }
        if !ops.is_empty() {
            s.append_item(&super::menuicons::model_submenu("_Drop As", &clone));
        }
        menu.append_section(None, &s);
    } else {
        let s = gio::Menu::new();
        s.append_section(None, &make);
        if !ops.is_empty() {
            s.append_section(None, &clone);
        }
        menu.append_item(&super::menuicons::model_submenu("_Drop As", &s));
    }
}

/// The menu a right-button drag of `sources` opens where it drops on `dest`.
pub fn drop_menu_model(sources: &[ClipSource], dest: &Location) -> gio::Menu {
    let menu = gio::Menu::new();
    let first = gio::Menu::new();
    first.append_item(&item("_Move here", "drop.do", "move"));
    first.append_item(&item("_Copy here", "drop.do", "copy"));
    menu.append_section(None, &first);
    let paths = local_paths(sources);
    if let (Some(dir), true) = (dest.local_path(), paths.len() == sources.len()) {
        let arch = gio::Menu::new();
        arch.append_item(&item("Create _Archive here…", "drop.do", "archive"));
        menu.append_section(None, &arch);
        link_sections(&paths, dir, "drop.do", &menu, true);
    }
    let last = gio::Menu::new();
    last.append_item(&item("C_ancel", "drop.do", "cancel"));
    menu.append_section(None, &last);
    menu
}

/// Opens the drop menu at (`x`, `y`) of `view` for `sources` dropped on `dest`.
pub fn show_drop_menu(w: &Window, view: &gtk::Widget, x: i32, y: i32, sources: Vec<ClipSource>, dest: Location) {
    let model = drop_menu_model(&sources, &dest);
    let group = gio::SimpleActionGroup::new();
    let a = gio::SimpleAction::new("do", Some(glib::VariantTy::STRING));
    let weak = Rc::downgrade(&w.me());
    a.connect_activate(move |_, v| {
        let (Some(w), Some(id)) = (weak.upgrade(), v.and_then(|v| v.get::<String>())) else { return };
        perform(&w, &id, &sources, &dest);
    });
    group.add_action(&a);
    let menu = gtk::Menu::from_model(&model);
    menu.insert_action_group("drop", Some(&group));
    menu.set_attach_widget(Some(view));
    if let Some(win) = view.window() {
        let rect = gtk::gdk::Rectangle::new(x, y, 1, 1);
        menu.popup_at_rect(&win, &rect, gtk::gdk::Gravity::NorthWest, gtk::gdk::Gravity::NorthWest, None);
    }
    OPEN.with(|o| *o.borrow_mut() = Some((model, group, menu)));
}

/// Does what the drop menu item or *Drop As* item `id` stands for.
pub fn perform(w: &Rc<Window>, id: &str, sources: &[ClipSource], dest: &Location) {
    let paths = local_paths(sources);
    let dir = dest.local_path().map(Path::to_path_buf);
    match id {
        "move" => fileops::transfer(w, sources.to_vec(), dest.clone(), Mode::Move, || {}),
        "copy" => fileops::transfer(w, sources.to_vec(), dest.clone(), Mode::Copy, || {}),
        "archive" => {
            if let Some(dir) = dir {
                super::compress::show_for(w, paths, dir);
            }
        }
        "delorean" => {
            if let (Some(dir), [one]) = (dir, paths.as_slice()) {
                delorean(w, one.clone(), dir);
            }
        }
        "cancel" => {}
        _ => {
            let Some(dir) = dir else { return };
            if id == "link-copy" {
                copy_links(w, &paths, &dir);
            } else if id == "link-mountpoint" {
                mount_volumes(w, paths, dir);
            } else if let Some(kind) = id.strip_prefix("link-").and_then(LinkKind::from_id) {
                make_links(w, kind, &paths, &dir);
            } else if let Some((_, op)) = TREE_OPS.iter().find(|(i, _)| *i == id) {
                run_tree(w, *op, paths, dir);
            }
        }
    }
}

fn make_links(w: &Window, kind: LinkKind, paths: &[PathBuf], dir: &Path) {
    let relative = w.app.settings.borrow().relative_symlinks;
    let mut made = Vec::new();
    let mut errors = Vec::new();
    for p in paths {
        let icon = super::desktoplink::icon_name_for_path(p);
        match links::make::create(kind, p, dir, relative, &icon) {
            Ok(l) => made.push(l),
            Err(e) => errors.push(format!("{}: {e}", p.file_name().unwrap_or_default().to_string_lossy())),
        }
    }
    if !errors.is_empty() {
        super::util::show_error(&w.win, &format!("Cannot create {}", kind.label().to_lowercase()), &errors.join("\n"));
    }
    fileops::select_created(w, &Location::Dir(dir.to_path_buf()), &made);
}

/// Volume Mountpoints of the drives `paths` (their top folders) in `dir`: a job, since the
/// administrator password is asked for meanwhile.
fn mount_volumes(w: &Window, paths: Vec<PathBuf>, dir: PathBuf) {
    let title = format!("Mounting {} in “{}”", count(paths.len()), folder_name(&dir));
    let weak = Rc::downgrade(&w.me());
    let d2 = dir.clone();
    w.app.jobs.start(
        w.win.upcast_ref(),
        title,
        move |ctx| {
            ctx.set_phase(failbrauwser::ops::job::Phase::Working);
            let admin = admin()?;
            let mut made = Vec::new();
            for p in &paths {
                match links::volume::create(p, &d2, &admin) {
                    Ok(f) => made.push(f),
                    Err(e) => ctx.record_error(format!("{}: {e}", p.display())),
                }
            }
            Ok(made)
        },
        move |outcome| {
            if let (Some(w), Outcome::Done(made)) = (weak.upgrade(), outcome) {
                fileops::select_created(&w, &Location::Dir(dir.clone()), made);
            }
        },
    );
}

/// *Unmount Volume*: the drive taken out of the Volume Mountpoint `folder`.
fn unmount_volume(w: &Window, folder: PathBuf) {
    let title = format!("Unmounting “{}”", folder_name(&folder));
    let weak = Rc::downgrade(&w.me());
    w.app.jobs.start(
        w.win.upcast_ref(),
        title,
        move |ctx| {
            ctx.set_phase(failbrauwser::ops::job::Phase::Working);
            links::volume::remove(&folder, &admin()?)?;
            Ok(Vec::new())
        },
        move |_| {
            if let Some(w) = weak.upgrade() {
                w.current_pane().reload();
            }
        },
    );
}

/// A tree run through the administrator helper, reporting into the job.
fn elevated(ctx: &failbrauwser::ops::job::JobCtx, args: Vec<std::ffi::OsString>) -> std::io::Result<tree::Report> {
    let mut step = progress(ctx);
    let reply = admin()?.run(args, &mut |p| step(Path::new(p)))?;
    Ok(reply.report())
}

fn copy_links(w: &Window, paths: &[PathBuf], dir: &Path) {
    let relative = w.app.settings.borrow().relative_symlinks;
    let (mut made, mut errors) = (Vec::new(), Vec::new());
    for p in paths {
        match links::make::copy_link(p, dir, relative) {
            Ok(l) => made.push(l),
            Err(e) => errors.push(format!("{}: {e}", p.display())),
        }
    }
    if !errors.is_empty() {
        super::util::show_error(&w.win, "Cannot copy the link", &errors.join("\n"));
    }
    fileops::select_created(w, &Location::Dir(dir.to_path_buf()), &made);
}

fn count(n: usize) -> String {
    if n == 1 { "1 item".into() } else { format!("{n} items") }
}

fn folder_name(dir: &Path) -> String {
    dir.file_name().map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// A progress callback for a tree run inside a job: names what it is at, counts it, waits while
/// paused and stops when cancelled.
fn progress(ctx: &failbrauwser::ops::job::JobCtx) -> impl FnMut(&Path) -> bool + '_ {
    use std::sync::atomic::Ordering;
    move |p: &Path| {
        ctx.wait_while_paused();
        ctx.set_current(p.file_name().unwrap_or_default().to_string_lossy().into_owned());
        ctx.files_done.fetch_add(1, Ordering::Relaxed);
        !ctx.is_cancelled()
    }
}

fn finish(ctx: &failbrauwser::ops::job::JobCtx, r: &tree::Report) -> std::io::Result<()> {
    for (p, e) in &r.errors {
        ctx.record_error(format!("{}: {e}", p.display()));
    }
    if r.cancelled {
        return Err(failbrauwser::ops::fastcopy::cancelled());
    }
    Ok(())
}

fn run_tree(w: &Window, op: tree::TreeOp, paths: Vec<PathBuf>, dir: PathBuf) {
    let opts = options(w);
    let title = format!("{} of {} to “{}”", op.label(), count(paths.len()), folder_name(&dir));
    let weak = Rc::downgrade(&w.me());
    let d2 = dir.clone();
    w.app.jobs.start(
        w.win.upcast_ref(),
        title,
        move |ctx| {
            ctx.set_phase(failbrauwser::ops::job::Phase::Working);
            let pairs = tree::destinations(&paths, &d2);
            let r = if through_helper(opts) {
                elevated(ctx, links::helper::tree_args(op, opts, &pairs))?
            } else {
                if opts.backup {
                    links::backup::enable_privileges()?;
                }
                tree::run(op, &pairs, opts, &mut progress(ctx))
            };
            finish(ctx, &r)?;
            Ok(pairs.into_iter().map(|(_, d)| d).collect())
        },
        move |outcome| {
            if let (Some(w), Outcome::Done(made)) = (weak.upgrade(), outcome) {
                fileops::select_created(&w, &Location::Dir(dir.clone()), made);
            }
        },
    );
}

fn delorean(w: &Window, source: PathBuf, dir: PathBuf) {
    let opts = options(w);
    let title = format!("DeLorean Copy of “{}” to “{}”", folder_name(&source), folder_name(&dir));
    let weak = Rc::downgrade(&w.me());
    let d2 = dir.clone();
    w.app.jobs.start(
        w.win.upcast_ref(),
        title,
        move |ctx| {
            ctx.set_phase(failbrauwser::ops::job::Phase::Working);
            let stamp = tree::stamp_now();
            let (made, r) = if through_helper(opts) {
                let name = source.file_name().unwrap_or_default().to_string_lossy().into_owned();
                (d2.join(format!("{name} - {stamp}")), elevated(ctx, links::helper::delorean_args(opts, &stamp, &source, &d2))?)
            } else {
                if opts.backup {
                    links::backup::enable_privileges()?;
                }
                tree::delorean(&source, &d2, &stamp, opts, &mut progress(ctx))?
            };
            finish(ctx, &r)?;
            Ok(vec![made])
        },
        move |outcome| {
            if let (Some(w), Outcome::Done(made)) = (weak.upgrade(), outcome) {
                fileops::select_created(&w, &Location::Dir(dir.clone()), made);
            }
        },
    );
}

/// Smart Move after `pairs` (from, to) were moved: the links inside moved folders reach what
/// they reached before (README: Links); in Backup Mode through the administrator helper. Runs
/// where it is called (a worker thread).
pub fn smart_move(pairs: &[(PathBuf, PathBuf)], how: SmartMove) -> Vec<String> {
    let folders: Vec<(PathBuf, PathBuf)> = pairs.iter().filter(|(_, to)| std::fs::symlink_metadata(to).is_ok_and(|m| m.is_dir())).cloned().collect();
    if folders.is_empty() {
        return Vec::new();
    }
    if how.backup && cfg!(unix) {
        let reply = admin().and_then(|a| a.run(links::helper::smart_move_args(how.relative, &folders), &mut |_| true));
        return match reply {
            Ok(r) => r.errors.into_iter().map(|(p, e)| format!("{p}: {e}")).collect(),
            Err(e) => vec![format!("Smart Move: {e}")],
        };
    }
    let mut errors = Vec::new();
    for (from, to) in &folders {
        for (p, e) in links::moved::after_move(from, to, how.relative, how.backup).errors {
            errors.push(format!("{}: {e}", p.display()));
        }
    }
    errors
}

/// Window actions: *Pick Link Source*, *Drop As*, *Replace Symbolic Link / Junction*, *Cancel
/// Link Creation*.
pub fn install_actions(w: &Window) {
    w.add_action("pick-link-source", |w| {
        let picked: Vec<PathBuf> = w.current_pane().selected_items().iter().filter_map(|i| i.path().cloned()).collect();
        PICKED.with(|p| *p.borrow_mut() = picked);
    });
    w.add_action("cancel-link-creation", |_| PICKED.with(|p| p.borrow_mut().clear()));
    let a = gio::SimpleAction::new("drop-as", Some(glib::VariantTy::STRING));
    let weak = Rc::downgrade(&w.me());
    a.connect_activate(move |_, v| {
        let (Some(w), Some(id)) = (weak.upgrade(), v.and_then(|v| v.get::<String>())) else { return };
        let picked = PICKED.with(|p| std::mem::take(&mut *p.borrow_mut()));
        let Some(dest) = drop_as_target(&w) else { return };
        perform(&w, &id, &picked.into_iter().map(ClipSource::Local).collect::<Vec<_>>(), &dest);
    });
    w.win.add_action(&a);
    w.add_action("unmount-mountpoint", |w| {
        if let Some(folder) = selected_mountpoint(w) {
            unmount_volume(w, folder);
        }
    });
    w.add_action("replace-link", |w| {
        let picked = PICKED.with(|p| p.borrow().clone());
        let (Some(link), [target]) = (selected_link(w), picked.as_slice()) else { return };
        let how = SmartMove::of(w);
        if how.backup && cfg!(unix) {
            replace_elevated(w, link, target.clone(), how.relative);
            return;
        }
        match links::make::retarget_with(&link, target, how.relative, how.backup) {
            Ok(()) => {
                PICKED.with(|p| p.borrow_mut().clear());
                w.current_pane().reload();
            }
            Err(e) => super::util::show_error(&w.win, "Cannot replace the link", &e.to_string()),
        }
    });
}

/// *Replace Symbolic Link / Junction* in Backup Mode: through the administrator helper, as a job
/// (the password is asked for meanwhile).
fn replace_elevated(w: &Window, link: PathBuf, target: PathBuf, relative: bool) {
    let title = format!("Replacing “{}”", folder_name(&link));
    let weak = Rc::downgrade(&w.me());
    w.app.jobs.start(
        w.win.upcast_ref(),
        title,
        move |ctx| {
            ctx.set_phase(failbrauwser::ops::job::Phase::Working);
            admin()?.run(links::helper::retarget_args(relative, &link, &target), &mut |_| true)?.result()?;
            Ok(Vec::new())
        },
        move |outcome| {
            if let (Some(w), Outcome::Done(_)) = (weak.upgrade(), outcome) {
                PICKED.with(|p| p.borrow_mut().clear());
                w.current_pane().reload();
            }
        },
    );
}

/// The one selected item when it is a Volume Mountpoint that can be unmounted.
fn selected_mountpoint(w: &Window) -> Option<PathBuf> {
    match w.current_pane().selected_items().as_slice() {
        [Item::Fs(e)] if e.kind == failbrauwser::fs::Kind::Dir && links::volume::is_mountpoint(&e.path) => Some(e.path.clone()),
        _ => None,
    }
}

/// Where *Drop As* drops: into the one folder selected, else into the folder shown.
fn drop_as_target(w: &Window) -> Option<Location> {
    let p = w.current_pane();
    match p.selected_items().as_slice() {
        [one] if one.is_dir_like() => one.path().map(|d| Location::Dir(d.clone())),
        [] => matches!(p.location(), Location::Dir(_)).then(|| p.location()),
        _ => None,
    }
}

/// The one selected item when it is a symbolic link or junction to a folder.
fn selected_link(w: &Window) -> Option<PathBuf> {
    match w.current_pane().selected_items().as_slice() {
        [Item::Fs(e)] if e.kind == failbrauwser::fs::Kind::Symlink => Some(e.path.clone()),
        _ => None,
    }
}

/// The links section of the right-click menu (README: Links, *Pick and drop without dragging*).
pub fn menu_section(w: &Window, items: &[Item]) -> Option<gio::Menu> {
    let s = gio::Menu::new();
    let local: Vec<PathBuf> = items.iter().filter_map(|i| i.path().cloned()).collect();
    if !items.is_empty() && local.len() == items.len() {
        s.append_item(&super::menuicons::model_item("Pic_k Link Source", "win.pick-link-source"));
    }
    if selected_mountpoint(w).is_some() {
        s.append_item(&super::menuicons::model_item("_Unmount Volume", "win.unmount-mountpoint"));
    }
    let picked = PICKED.with(|p| p.borrow().clone());
    if !picked.is_empty() {
        if let Some(dir) = drop_as_target(w).and_then(|l| l.local_path().map(Path::to_path_buf)) {
            link_sections(&picked, &dir, "win.drop-as", &s, false);
        }
        if let (Some(link), [target]) = (selected_link(w), picked.as_slice()) {
            if target.is_dir() {
                let label = if links::make::is_junction(&link) { "Replace _Junction" } else { "Replace _Symbolic Link" };
                s.append_item(&super::menuicons::model_item(label, "win.replace-link"));
            }
        }
        s.append_item(&super::menuicons::model_item("Cancel Link _Creation", "win.cancel-link-creation"));
    }
    (s.n_items() > 0).then_some(s)
}

/// The labels of the drop menu open last, submenus as "Parent/Child" (self-test).
pub fn open_drop_menu_labels() -> Vec<String> {
    fn walk(m: &gio::MenuModel, prefix: &str, out: &mut Vec<String>) {
        for i in 0..m.n_items() {
            let label = m.item_attribute_value(i, "label", None).and_then(|v| v.get::<String>()).map(|l| super::menuicons::plain_text(&l));
            if let Some(sec) = m.item_link(i, "section") {
                walk(&sec, prefix, out);
            }
            if let Some(l) = &label {
                let full = format!("{prefix}{l}");
                if let Some(sub) = m.item_link(i, "submenu") {
                    walk(&sub, &format!("{full}/"), out);
                }
                out.push(full);
            }
        }
    }
    OPEN.with(|o| {
        let mut out = Vec::new();
        if let Some((m, ..)) = o.borrow().as_ref() {
            walk(m.upcast_ref(), "", &mut out);
        }
        out
    })
}

/// Clicks the item `path` ("Create Link here/Hard Link") of the drop menu open last (self-test).
pub fn click_open_drop_menu(path: &str) -> Result<(), String> {
    fn find(m: &gio::MenuModel, prefix: &str, want: &str) -> Option<String> {
        for i in 0..m.n_items() {
            if let Some(sec) = m.item_link(i, "section") {
                if let Some(t) = find(&sec, prefix, want) {
                    return Some(t);
                }
            }
            let Some(label) = m.item_attribute_value(i, "label", None).and_then(|v| v.get::<String>()) else { continue };
            let full = format!("{prefix}{}", super::menuicons::plain_text(&label));
            if let Some(sub) = m.item_link(i, "submenu") {
                if let Some(t) = find(&sub, &format!("{full}/"), want) {
                    return Some(t);
                }
            }
            if full == want {
                return m.item_attribute_value(i, "target", None).and_then(|v| v.get::<String>());
            }
        }
        None
    }
    OPEN.with(|o| {
        let o = o.borrow();
        let (m, group, menu) = o.as_ref().ok_or("no drop menu is open")?;
        let id = find(m.upcast_ref(), "", path).ok_or_else(|| format!("no item {path:?}"))?;
        menu.popdown();
        group.activate_action("do", Some(&id.to_variant()));
        Ok(())
    })
}
