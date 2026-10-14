//! The side panel: shortcuts (Drives, Home, Desktop, Trash, the user's folders and the
//! mounted volumes, arranged by the user; see [`failbrauwser::shortcuts`]) above a folder tree.
//!
//! Right-click an entry to rename, move, hide or remove it; drag entries to reorder them and
//! drop folders onto the list to add them. Every window shows the same entries.

use super::util;
use super::window::Window;
use failbrauwser::config::TreeRoot;
use failbrauwser::location::Location;
use failbrauwser::shortcuts::{Kind, Shortcuts};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

const T_ICON: u32 = 0;
const T_NAME: u32 = 1;
const T_PATH: u32 = 2;
/// Children have been read (a placeholder child is shown until then).
const T_LOADED: u32 = 3;

/// What a shortcut row stands for.
#[derive(Clone)]
enum Target {
    Place(Location),
    Mount(gio::Mount),
}

struct RowInfo {
    /// Index in the shortcut list (a mount row: the "Mounted volumes" entry's index).
    index: usize,
    target: Target,
    label: String,
}

pub struct Sidebar {
    /// The shortcuts. Rows are marked by hand while their place is shown: a selectable list
    /// would select its row as soon as it gets the keyboard focus.
    list: gtk::ListBox,
    rows: RefCell<Vec<RowInfo>>,
    tree: gtk::TreeView,
    store: gtk::TreeStore,
    tree_box: gtk::ScrolledWindow,
    split: gtk::Paned,
    window: Weak<Window>,
    root: RefCell<Option<PathBuf>>,
    /// Ignore selection changes we cause ourselves while revealing the current folder.
    syncing: std::cell::Cell<bool>,
}

thread_local! {
    static SIDEBARS: RefCell<Vec<(Weak<Window>, Rc<Sidebar>)>> = const { RefCell::new(Vec::new()) };
    static SHORTCUTS: RefCell<Option<Shortcuts>> = const { RefCell::new(None) };
    static MONITOR: RefCell<Option<gio::VolumeMonitor>> = const { RefCell::new(None) };
}

/// Marks a shortcut row being dragged within the list.
const ROW_TARGET: &str = "application/x-failbrauwser-shortcut";
const URI_LIST: &str = "text/uri-list";

fn shortcuts() -> Shortcuts {
    SHORTCUTS.with(|s| s.borrow_mut().get_or_insert_with(Shortcuts::load).clone())
}

/// Changes the shortcuts, saves them and shows the change in every window.
fn edit_shortcuts(f: impl FnOnce(&mut Shortcuts)) {
    let mut sc = shortcuts();
    f(&mut sc);
    if let Err(e) = sc.save() {
        eprintln!("failbrauwser: cannot save the side panel entries: {e}");
    }
    SHORTCUTS.with(|s| *s.borrow_mut() = Some(sc));
    rebuild_all();
}

fn rebuild_all() {
    let all: Vec<Rc<Sidebar>> = SIDEBARS.with(|s| s.borrow().iter().map(|(_, sb)| sb.clone()).collect());
    for sb in all {
        sb.rebuild();
    }
}

/// Mounts and unmounts show up in every panel.
fn watch_mounts() {
    MONITOR.with(|m| {
        if m.borrow().is_some() {
            return;
        }
        let mon = gio::VolumeMonitor::get();
        mon.connect_mount_added(|_, _| rebuild_all());
        mon.connect_mount_removed(|_, _| rebuild_all());
        mon.connect_mount_changed(|_, _| rebuild_all());
        *m.borrow_mut() = Some(mon);
    });
}

/// Mounts worth a row: not hidden behind another one, and not the system's own file systems.
fn visible_mounts() -> Vec<gio::Mount> {
    gio::VolumeMonitor::get().mounts().into_iter().filter(|m| !m.is_shadowed()).collect()
}

fn themed(names: &[&str]) -> gio::Icon {
    gio::ThemedIcon::from_names(names).upcast()
}

fn place_icon(kind: &Kind) -> gio::Icon {
    match kind {
        Kind::Drives => themed(&["drive-harddisk-symbolic", "drive-harddisk"]),
        Kind::Home => themed(&["user-home-symbolic", "user-home"]),
        Kind::Desktop => themed(&["user-desktop-symbolic", "user-desktop"]),
        Kind::Trash => themed(&["user-trash-symbolic", "user-trash"]),
        Kind::Mounts => themed(&["drive-removable-media-symbolic", "drive-removable-media"]),
        Kind::Dir(p) => util::place_icon(p),
    }
}

fn place_location(kind: &Kind) -> Location {
    match kind {
        Kind::Drives => Location::Drives,
        Kind::Home => Location::home(),
        Kind::Desktop => Location::Dir(glib::user_special_dir(glib::UserDirectory::Desktop).unwrap_or_else(|| glib::home_dir().join("Desktop"))),
        Kind::Trash => Location::Trash,
        Kind::Mounts => Location::Drives,
        Kind::Dir(p) => Location::Dir(p.clone()),
    }
}

/// Adds the folders among `dirs` at shortcut index `at`; whether any was a folder.
fn add_dirs(dirs: Vec<PathBuf>, at: usize) -> bool {
    let dirs: Vec<PathBuf> = dirs.into_iter().filter(|p| p.is_dir()).collect();
    if dirs.is_empty() {
        return false;
    }
    edit_shortcuts(|s| {
        let mut at = at;
        for d in dirs {
            at = s.add_dir(d, Some(at)) + 1;
        }
    });
    true
}

fn make_row(icon: &gio::Icon, label: &str, tooltip: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let image = gtk::Image::from_gicon(icon, gtk::IconSize::Menu);
    image.style_context().add_class("sidebar-icon");
    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.style_context().add_class("sidebar-label");
    content.pack_start(&image, false, false, 0);
    content.pack_start(&text, true, true, 0);
    row.add(&content);
    if !tooltip.is_empty() {
        row.set_tooltip_text(Some(tooltip));
    }
    row
}

/// The sidebar belonging to a window.
pub fn for_window(w: &Window) -> Option<Rc<Sidebar>> {
    SIDEBARS.with(|s| {
        let mut s = s.borrow_mut();
        s.retain(|(w, _)| w.strong_count() > 0);
        s.iter().find(|(x, _)| x.upgrade().is_some_and(|x| std::ptr::eq(&*x, w))).map(|(_, sb)| sb.clone())
    })
}

pub fn attach(w: &Rc<Window>) {
    let store = gtk::TreeStore::new(&[gio::Icon::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::BOOL]);
    let tree = gtk::TreeView::with_model(&store);
    tree.set_headers_visible(false);
    tree.set_enable_search(true);
    tree.set_search_column(T_NAME as i32);
    tree.set_show_expanders(true);
    tree.set_level_indentation(0);
    let col = gtk::TreeViewColumn::new();
    let pix = gtk::CellRendererPixbuf::new();
    let text = gtk::CellRendererText::new();
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    TreeViewColumnExt::pack_start(&col, &pix, false);
    TreeViewColumnExt::add_attribute(&col, &pix, "gicon", T_ICON as i32);
    TreeViewColumnExt::pack_start(&col, &text, true);
    TreeViewColumnExt::add_attribute(&col, &text, "text", T_NAME as i32);
    tree.append_column(&col);
    let tree_box = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    tree_box.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    tree_box.add(&tree);

    let list = gtk::ListBox::new();
    list.style_context().add_class("sidebar");
    list.style_context().add_class("fb-shortcuts");
    list.set_selection_mode(gtk::SelectionMode::None);
    list.set_activate_on_single_click(true);
    let top = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    top.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    top.add(&list);
    top.style_context().add_class("sidebar");

    let split = gtk::Paned::new(gtk::Orientation::Vertical);
    split.pack1(&top, true, false);
    split.pack2(&tree_box, true, false);
    split.set_position(w.app.settings.borrow().sidebar_split);
    w.side.pack_start(&split, true, true, 0);
    w.side.show_all();

    let sb = Rc::new(Sidebar {
        list,
        rows: RefCell::new(Vec::new()),
        tree,
        store,
        tree_box,
        split,
        window: Rc::downgrade(w),
        root: RefCell::new(None),
        syncing: std::cell::Cell::new(false),
    });
    sb.connect();
    SIDEBARS.with(|s| s.borrow_mut().push((Rc::downgrade(w), sb.clone())));
    watch_mounts();
    sb.rebuild();
    sb.apply_settings();
    let weak = Rc::downgrade(&sb);
    w.connect_location_changed(move |_| {
        if let Some(sb) = weak.upgrade() {
            sb.location_changed();
        }
    });
}

impl Sidebar {
    /// Activates the Drives entry as a click would (tests).
    pub fn click_drives(&self) {
        if let Some(r) = self.row_showing(&Location::Drives) {
            r.activate();
        }
    }

    pub fn drives_selected(&self) -> bool {
        self.row_showing(&Location::Drives).is_some_and(|r| r.state_flags().contains(gtk::StateFlags::SELECTED))
    }

    fn row_showing(&self, loc: &Location) -> Option<gtk::ListBoxRow> {
        let i = self.rows.borrow().iter().position(|r| matches!(&r.target, Target::Place(l) if l == loc))?;
        self.list.row_at_index(i as i32)
    }

    /// Fills the list from the shortcuts (and the current mounts).
    fn rebuild(&self) {
        for child in self.list.children() {
            self.list.remove(&child);
        }
        let mut rows = Vec::new();
        for (index, sc) in shortcuts().0.iter().enumerate() {
            if sc.hidden {
                continue;
            }
            if sc.kind == Kind::Mounts {
                for m in visible_mounts() {
                    let tip = m.root().path().map(|p| p.display().to_string()).unwrap_or_else(|| m.root().uri().to_string());
                    let row = make_row(&m.symbolic_icon(), &m.name(), &tip);
                    self.list.add(&row);
                    rows.push(RowInfo { index, label: m.name().to_string(), target: Target::Mount(m) });
                }
                continue;
            }
            let loc = place_location(&sc.kind);
            let tip = match &sc.kind {
                Kind::Drives => "All drives with their fill levels (Alt+D)".to_string(),
                Kind::Trash => String::new(),
                _ => loc.display(),
            };
            let row = make_row(&place_icon(&sc.kind), &sc.label(), &tip);
            if let Kind::Dir(p) = &sc.kind {
                // Never asked on a network share here: one that hangs would hang the window.
                if !failbrauwser::ops::gvfs::is_gvfs_path(p) && !p.is_dir() {
                    // Gone (or on a drive that is not there now): still listed, dimmed.
                    row.set_opacity(0.5);
                }
            }
            self.list.add(&row);
            rows.push(RowInfo { index, label: sc.label(), target: Target::Place(loc) });
        }
        for row in self.list.children() {
            if let Ok(row) = row.downcast::<gtk::ListBoxRow>() {
                self.enable_row_drag(&row);
            }
        }
        *self.rows.borrow_mut() = rows;
        self.list.show_all();
        self.mark_current();
    }

    /// Marks the row of the place the window shows.
    fn mark_current(&self) {
        let Some(w) = self.window.upgrade() else { return };
        let loc = w.current_location();
        let rows = self.rows.borrow();
        for (i, info) in rows.iter().enumerate() {
            let Some(row) = self.list.row_at_index(i as i32) else { continue };
            let here = match &info.target {
                Target::Place(l) => *l == loc,
                Target::Mount(m) => m.root().path().is_some_and(|p| loc.local_path() == Some(p.as_path())),
            };
            if here {
                row.set_state_flags(gtk::StateFlags::SELECTED, false);
            } else {
                row.unset_state_flags(gtk::StateFlags::SELECTED);
            }
        }
    }

    fn open(&self, target: &Target, new_tab: bool) {
        let Some(w) = self.window.upgrade() else { return };
        let loc = match target {
            Target::Place(l) => l.clone(),
            Target::Mount(m) => match m.root().path() {
                Some(p) => Location::Dir(p),
                None => {
                    super::extensions::open_uri(&w, &m.root().uri());
                    return;
                }
            },
        };
        if new_tab {
            w.add_tab(loc, true);
        } else {
            w.navigate(loc);
        }
    }

    fn info_at(&self, row: &gtk::ListBoxRow) -> Option<(usize, Target)> {
        let i = usize::try_from(row.index()).ok()?;
        self.rows.borrow().get(i).map(|r| (r.index, r.target.clone()))
    }

    fn context_menu(self: &Rc<Self>, row: Option<&gtk::ListBoxRow>, ev: &gdk::EventButton) {
        let menu = self.menu_for(row);
        menu.popup_at_pointer(Some(ev));
    }

    /// The right-click menu of a row (`None`: the empty space below the rows).
    fn menu_for(self: &Rc<Self>, row: Option<&gtk::ListBoxRow>) -> gtk::Menu {
        let menu = gtk::Menu::new();
        let add = |menu: &gtk::Menu, label: &str, enabled: bool, f: Box<dyn Fn()>| {
            let item = gtk::MenuItem::with_mnemonic(label);
            item.set_sensitive(enabled);
            item.connect_activate(move |_| f());
            menu.append(&item);
        };
        let sc = shortcuts();
        let visible: Vec<usize> = sc.0.iter().enumerate().filter(|(_, s)| !s.hidden).map(|(i, _)| i).collect();
        if let Some((index, target)) = row.and_then(|r| self.info_at(r)) {
            let entry = sc.0[index].clone();
            let me = Rc::downgrade(self);
            let t = target.clone();
            add(&menu, "Open in New _Tab", true, Box::new(move || {
                if let Some(sb) = me.upgrade() {
                    sb.open(&t, true);
                }
            }));
            if let Target::Mount(m) = &target {
                menu.append(&gtk::SeparatorMenuItem::new());
                let me = Rc::downgrade(self);
                let mnt = m.clone();
                let label = if m.can_eject() { "_Eject" } else { "_Unmount" };
                add(&menu, label, m.can_eject() || m.can_unmount(), Box::new(move || {
                    if let Some(sb) = me.upgrade() {
                        sb.unmount(&mnt);
                    }
                }));
            }
            menu.append(&gtk::SeparatorMenuItem::new());
            if !matches!(target, Target::Mount(_)) {
                let me = Rc::downgrade(self);
                let current = entry.label();
                add(&menu, "_Rename…", true, Box::new(move || {
                    let Some(sb) = me.upgrade() else { return };
                    let Some(w) = sb.window.upgrade() else { return };
                    util::ask_text(&w.win, "Rename Entry", "Name in the side panel (empty for the default):", &current, "_Rename", false, move |name| {
                        edit_shortcuts(|s| s.rename(index, &name));
                    });
                }));
            }
            // Up and down skip hidden entries: they move past what is seen.
            let pos = visible.iter().position(|&i| i == index).unwrap_or(0);
            let prev = pos.checked_sub(1).map(|p| visible[p]);
            let next = visible.get(pos + 1).copied();
            add(&menu, "Move _Up", prev.is_some(), Box::new(move || {
                if let Some(to) = prev {
                    edit_shortcuts(|s| s.move_to(index, to));
                }
            }));
            add(&menu, "Move _Down", next.is_some(), Box::new(move || {
                if let Some(to) = next {
                    edit_shortcuts(|s| s.move_to(index, to));
                }
            }));
            menu.append(&gtk::SeparatorMenuItem::new());
            let hide_label = if entry.kind == Kind::Mounts { "_Hide Mounted Volumes" } else { "_Hide" };
            add(&menu, hide_label, true, Box::new(move || edit_shortcuts(|s| s.set_hidden(index, true))));
            if !entry.kind.is_builtin() {
                add(&menu, "Re_move", true, Box::new(move || edit_shortcuts(|s| {
                    s.remove(index);
                })));
            }
            menu.append(&gtk::SeparatorMenuItem::new());
        }
        let current_dir = self.window.upgrade().and_then(|w| w.current_location().local_path().map(Path::to_path_buf));
        let shown = current_dir.as_ref().is_some_and(|d| sc.0.iter().any(|s| !s.hidden && s.kind == Kind::Dir(d.clone())));
        let at = row.and_then(|r| self.info_at(r)).map(|(i, _)| i + 1);
        add(&menu, "_Add Current Folder", current_dir.is_some() && !shown, Box::new(move || {
            if let Some(d) = current_dir.clone() {
                edit_shortcuts(|s| {
                    s.add_dir(d, at);
                });
            }
        }));
        let hidden: Vec<(usize, String)> = sc.0.iter().enumerate().filter(|(_, s)| s.hidden).map(|(i, s)| (i, s.label())).collect();
        let show = gtk::MenuItem::with_mnemonic("_Show Hidden Entry");
        if hidden.is_empty() {
            show.set_sensitive(false);
        } else {
            let sub = gtk::Menu::new();
            for (i, label) in hidden {
                let item = gtk::MenuItem::with_label(&label);
                item.connect_activate(move |_| edit_shortcuts(|s| s.set_hidden(i, false)));
                sub.append(&item);
            }
            show.set_submenu(Some(&sub));
        }
        menu.append(&show);
        menu.show_all();
        menu
    }

    fn unmount(&self, m: &gio::Mount) {
        let win = self.window.clone();
        let op = self.window.upgrade().map(|w| gtk::MountOperation::new(Some(&w.win)));
        let done = move |res: Result<(), glib::Error>| {
            if let (Err(e), Some(w)) = (res, win.upgrade()) {
                if !e.matches(gio::IOErrorEnum::FailedHandled) {
                    util::show_error(&w.win, "Cannot unmount", &e.to_string());
                }
            }
        };
        if m.can_eject() {
            m.eject_with_operation(gio::MountUnmountFlags::NONE, op.as_ref(), gio::Cancellable::NONE, done);
        } else {
            m.unmount_with_operation(gio::MountUnmountFlags::NONE, op.as_ref(), gio::Cancellable::NONE, done);
        }
    }

    /// Rows can be dragged to another place in the list.
    fn enable_row_drag(&self, row: &gtk::ListBoxRow) {
        let targets = [gtk::TargetEntry::new(ROW_TARGET, gtk::TargetFlags::SAME_APP, 0)];
        row.drag_source_set(gdk::ModifierType::BUTTON1_MASK, &targets, gdk::DragAction::MOVE);
        row.connect_drag_begin(|row, ctx| {
            // The row itself as the drag image.
            let alloc = row.allocation();
            let surface = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, alloc.width(), alloc.height());
            if let Ok(surface) = surface {
                if let Ok(cr) = gtk::cairo::Context::new(&surface) {
                    row.draw(&cr);
                }
                ctx.drag_set_icon_surface(&surface);
            }
        });
        row.connect_drag_data_get(|row, _, sel, _, _| {
            sel.set(&gdk::Atom::intern(ROW_TARGET), 8, row.index().to_string().as_bytes());
        });
    }

    /// Where a drop at `y` goes: before the row under it (after it in its lower half), as an
    /// index in the shortcut list.
    fn drop_index(&self, y: i32) -> usize {
        let rows = self.rows.borrow();
        let Some(row) = self.list.row_at_y(y) else { return shortcuts().0.len() };
        let Some(info) = usize::try_from(row.index()).ok().and_then(|i| rows.get(i)) else { return shortcuts().0.len() };
        let alloc = row.allocation();
        if y > alloc.y() + alloc.height() / 2 { info.index + 1 } else { info.index }
    }

    fn connect_drops(self: &Rc<Self>) {
        let targets = [
            gtk::TargetEntry::new(ROW_TARGET, gtk::TargetFlags::SAME_APP, 0),
            gtk::TargetEntry::new(URI_LIST, gtk::TargetFlags::empty(), 1),
        ];
        self.list.drag_dest_set(gtk::DestDefaults::empty(), &targets, gdk::DragAction::MOVE | gdk::DragAction::COPY | gdk::DragAction::LINK);
        let offers = |ctx: &gdk::DragContext, name: &str| ctx.list_targets().iter().any(|a| a.name() == name);
        self.list.connect_drag_motion(move |list, ctx, _, y, time| {
            let action = if offers(ctx, ROW_TARGET) { gdk::DragAction::MOVE } else if offers(ctx, URI_LIST) { gdk::DragAction::LINK } else {
                ctx.drag_status(gdk::DragAction::empty(), time);
                return false;
            };
            match list.row_at_y(y) {
                Some(row) => list.drag_highlight_row(&row),
                None => list.drag_unhighlight_row(),
            }
            ctx.drag_status(action, time);
            true
        });
        self.list.connect_drag_leave(|list, _, _| list.drag_unhighlight_row());
        self.list.connect_drag_drop(move |list, ctx, _, _, time| {
            let want = if offers(ctx, ROW_TARGET) { ROW_TARGET } else if offers(ctx, URI_LIST) { URI_LIST } else { return false };
            list.drag_get_data(ctx, &gdk::Atom::intern(want), time);
            true
        });
        let me = Rc::downgrade(self);
        self.list.connect_drag_data_received(move |list, ctx, _, y, sel, _, time| {
            list.drag_unhighlight_row();
            let Some(sb) = me.upgrade() else { return };
            let at = sb.drop_index(y);
            let ok = if sel.target().name() == ROW_TARGET {
                String::from_utf8_lossy(&sel.data()).parse::<usize>().is_ok_and(|r| sb.move_row(r, at))
            } else {
                let dirs: Vec<PathBuf> = sel.uris().iter().filter_map(|u| glib::filename_from_uri(u).ok().map(|(p, _)| p)).collect();
                add_dirs(dirs, at)
            };
            ctx.drag_finish(ok, false, time);
        });
    }

    /// Moves the entry of list row `row` to shortcut index `at` (a drop position).
    fn move_row(&self, row: usize, at: usize) -> bool {
        let Some(from) = self.rows.borrow().get(row).map(|i| i.index) else { return false };
        let to = if from < at { at - 1 } else { at };
        edit_shortcuts(|s| s.move_to(from, to));
        true
    }

    // ── For the self-test ──

    fn row_labelled(&self, label: &str) -> Result<(usize, gtk::ListBoxRow), String> {
        let i = self.rows.borrow().iter().position(|r| r.label == label).ok_or_else(|| format!("no side panel entry {label:?}"))?;
        Ok((i, self.list.row_at_index(i as i32).ok_or("row missing")?))
    }

    /// The labels of the shortcut rows (mounted volumes left out: they depend on the machine).
    pub fn labels(&self) -> Vec<String> {
        self.rows.borrow().iter().filter(|r| matches!(r.target, Target::Place(_))).map(|r| r.label.clone()).collect()
    }

    /// The labels of the rows marked as the current place.
    pub fn marked(&self) -> Vec<String> {
        let rows = self.rows.borrow();
        (0..rows.len())
            .filter(|&i| self.list.row_at_index(i as i32).is_some_and(|r| r.state_flags().contains(gtk::StateFlags::SELECTED)))
            .map(|i| rows[i].label.clone())
            .collect()
    }

    pub fn click_label(&self, label: &str) -> Result<(), String> {
        self.row_labelled(label)?.1.activate();
        Ok(())
    }

    /// Activates an item of an entry's right-click menu (`label` empty: the menu of the empty
    /// space). `item` is the menu text without mnemonics; `Show Hidden Entry/<name>` picks
    /// from the submenu.
    pub fn menu_item(self: &Rc<Self>, label: &str, item: &str) -> Result<(), String> {
        let row = if label.is_empty() { None } else { Some(self.row_labelled(label)?.1) };
        let menu = self.menu_for(row.as_ref());
        let (first, rest) = item.split_once('/').map_or((item, None), |(a, b)| (a, Some(b)));
        let find = |menu: &gtk::Menu, text: &str| -> Option<gtk::MenuItem> {
            menu.children().into_iter().filter_map(|c| c.downcast::<gtk::MenuItem>().ok()).find(|m| m.label().is_some_and(|l| l.replace('_', "") == text))
        };
        let mut found = find(&menu, first).ok_or_else(|| format!("no menu item {first:?}"))?;
        if let Some(rest) = rest {
            let sub = found.submenu().and_then(|m| m.downcast::<gtk::Menu>().ok()).ok_or("no submenu")?;
            found = find(&sub, rest).ok_or_else(|| format!("no submenu item {rest:?}"))?;
        }
        if !found.is_sensitive() {
            return Err(format!("menu item {item:?} is disabled"));
        }
        found.activate();
        Ok(())
    }

    /// Drops an entry (as a drag would) before the entry `before` (empty: at the end).
    pub fn drag_before(&self, label: &str, before: &str) -> Result<(), String> {
        let (row, _) = self.row_labelled(label)?;
        let at = self.index_before(before)?;
        self.move_row(row, at);
        Ok(())
    }

    /// Drops folders (as from the file list) before the entry `before` (empty: at the end).
    pub fn drop_dirs_before(&self, dirs: Vec<PathBuf>, before: &str) -> Result<(), String> {
        let at = self.index_before(before)?;
        if add_dirs(dirs, at) { Ok(()) } else { Err("nothing was added".into()) }
    }

    fn index_before(&self, before: &str) -> Result<usize, String> {
        if before.is_empty() {
            return Ok(shortcuts().0.len());
        }
        let (i, _) = self.row_labelled(before)?;
        Ok(self.rows.borrow()[i].index)
    }

    /// The folder at the top of the tree.
    pub fn root_path(&self) -> Option<PathBuf> {
        self.root.borrow().clone()
    }

    /// The folder selected in the tree.
    pub fn selected_path(&self) -> Option<PathBuf> {
        let (model, iter) = self.tree.selection().selected()?;
        let p: String = model.value(&iter, T_PATH as i32).get().ok()?;
        Some(PathBuf::from(p))
    }

    fn tree_path_of(&self, dir: &Path) -> Option<gtk::TreePath> {
        let mut found = None;
        self.store.foreach(|m, path, iter| {
            let p: String = m.value(iter, T_PATH as i32).get().unwrap_or_default();
            if Path::new(&p) == dir {
                found = Some(path.clone());
                return true;
            }
            false
        });
        found
    }

    /// Re-reads the subfolders of `dir` if the tree already shows them (after changes).
    pub fn refresh_dir(&self, dir: &Path) {
        let Some(p) = self.tree_path_of(dir) else { return };
        let Some(iter) = self.store.iter(&p) else { return };
        let loaded: bool = self.store.value(&iter, T_LOADED as i32).get().unwrap_or(false);
        let has_children = self.store.iter_children(Some(&iter)).is_some();
        if !loaded || !has_children && !self.tree.row_expanded(&p) {
            // Never read, or known to be empty and closed: read when opened.
            if !has_children {
                self.add_placeholder(&iter);
                self.store.set_value(&iter, T_LOADED, &false.to_value());
            }
            return;
        }
        self.store.set_value(&iter, T_LOADED, &false.to_value());
        self.load_children(&iter, None);
    }

    /// The subfolder names the tree shows below `dir` (tests).
    pub fn children_of(&self, dir: &Path) -> Vec<String> {
        let Some(iter) = self.tree_path_of(dir).and_then(|p| self.store.iter(&p)) else { return Vec::new() };
        let mut out = Vec::new();
        let mut child = self.store.iter_children(Some(&iter));
        while let Some(c) = child {
            let path: String = self.store.value(&c, T_PATH as i32).get().unwrap_or_default();
            if !path.is_empty() {
                out.push(self.store.value(&c, T_NAME as i32).get().unwrap_or_default());
            }
            child = if self.store.iter_next(&c) { Some(c) } else { None };
        }
        out
    }

    /// Expands a folder once, like a click on its triangle (it must be shown).
    pub fn expand(&self, dir: &Path) -> bool {
        match self.tree_path_of(dir) {
            Some(p) => {
                self.tree.expand_row(&p, false);
                true
            }
            None => false,
        }
    }

    /// The folder is expanded and shows its real subfolders (not the placeholder).
    pub fn expanded_with_children(&self, dir: &Path) -> bool {
        let Some(p) = self.tree_path_of(dir) else { return false };
        let Some(iter) = self.store.iter(&p) else { return false };
        let first: Option<String> = self.store.iter_children(Some(&iter)).map(|c| self.store.value(&c, T_PATH as i32).get().unwrap_or_default());
        self.tree.row_expanded(&p) && first.is_some_and(|f| !f.is_empty())
    }

    /// Selects a folder in the tree as if clicked (it must be shown).
    pub fn click(&self, dir: &Path) -> bool {
        let mut found = None;
        self.store.foreach(|m, path, iter| {
            let p: String = m.value(iter, T_PATH as i32).get().unwrap_or_default();
            if Path::new(&p) == dir {
                found = Some(path.clone());
                return true;
            }
            false
        });
        match found {
            Some(p) => {
                // Like a real click: the cursor moves there too.
                self.tree.set_cursor(&p, None::<&gtk::TreeViewColumn>, false);
                true
            }
            None => false,
        }
    }

    fn connect(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            let Some(sb) = me.upgrade() else { return };
            if let Some((_, target)) = sb.info_at(row) {
                sb.open(&target, false);
            }
        });
        let me = Rc::downgrade(self);
        self.list.connect_button_press_event(move |list, ev| {
            let Some(sb) = me.upgrade() else { return glib::Propagation::Proceed };
            if ev.event_type() != gdk::EventType::ButtonPress {
                return glib::Propagation::Proceed;
            }
            let row = list.row_at_y(ev.position().1 as i32);
            match ev.button() {
                3 => {
                    sb.context_menu(row.as_ref(), ev);
                    glib::Propagation::Stop
                }
                2 => {
                    if let Some((_, target)) = row.as_ref().and_then(|r| sb.info_at(r)) {
                        sb.open(&target, true);
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.connect_drops();

        let me = Rc::downgrade(self);
        self.tree.connect_test_expand_row(move |_, iter, _| {
            if let Some(sb) = me.upgrade() {
                sb.load_children(iter, None);
            }
            glib::Propagation::Proceed
        });
        let me = Rc::downgrade(self);
        self.tree.selection().connect_changed(move |sel| {
            let Some(sb) = me.upgrade() else { return };
            if sb.syncing.get() {
                return;
            }
            let Some((model, iter)) = sel.selected() else { return };
            let path: String = model.value(&iter, T_PATH as i32).get().unwrap_or_default();
            if path.is_empty() {
                return;
            }
            // Navigating can rebuild this tree (a new root), which must not happen inside
            // the tree's own selection handling: GTK crashes clearing a store it is
            // still walking.
            let win = sb.window.clone();
            glib::idle_add_local_once(move || {
                if let Some(w) = win.upgrade() {
                    let target = Location::Dir(PathBuf::from(path));
                    if w.current_location() != target {
                        w.current_pane().navigate(target);
                    }
                }
            });
        });
        let me = Rc::downgrade(self);
        self.tree.connect_button_press_event(move |tree, ev| {
            let Some(sb) = me.upgrade() else { return glib::Propagation::Proceed };
            if ev.button() == 2 && ev.event_type() == gdk::EventType::ButtonPress {
                let (x, y) = ev.position();
                if let Some((Some(path), ..)) = tree.path_at_pos(x as i32, y as i32) {
                    if let Some(iter) = sb.store.iter(&path) {
                        let p: String = sb.store.value(&iter, T_PATH as i32).get().unwrap_or_default();
                        if let Some(w) = sb.window.upgrade() {
                            w.add_tab(Location::Dir(PathBuf::from(p)), false);
                        }
                    }
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        let win = self.window.clone();
        self.split.connect_position_notify(move |p| {
            if let Some(w) = win.upgrade() {
                w.app.settings.borrow_mut().sidebar_split = p.position();
                w.app.save_settings_soon();
            }
        });
    }

    /// Show or hide the tree and pick its root after a settings change.
    pub fn apply_settings(&self) {
        let Some(w) = self.window.upgrade() else { return };
        let show = w.app.settings.borrow().show_tree;
        self.tree_box.set_visible(show);
        *self.root.borrow_mut() = None;
        self.location_changed();
    }

    fn wanted_root(&self, current: Option<&Path>) -> Option<PathBuf> {
        let w = self.window.upgrade()?;
        let mode = w.app.settings.borrow().tree_root;
        match mode {
            TreeRoot::Home => Some(glib::home_dir()),
            TreeRoot::Filesystem => Some(PathBuf::from("/")),
            TreeRoot::Current => current.map(Path::to_path_buf),
        }
    }

    fn location_changed(&self) {
        let Some(w) = self.window.upgrade() else { return };
        let loc = w.current_location();
        let dir = loc.nearest_dir();
        self.mark_current();
        if !w.app.settings.borrow().show_tree {
            return;
        }
        let Some(root) = self.wanted_root(dir.as_deref()) else { return };
        if self.root.borrow().as_ref() != Some(&root) {
            self.set_root(&root);
        }
        if let Some(d) = dir {
            self.reveal(&d);
        }
    }

    fn set_root(&self, root: &Path) {
        *self.root.borrow_mut() = Some(root.to_path_buf());
        // Removing the selected row changes the selection; that is not a click.
        let was = self.syncing.replace(true);
        self.store.clear();
        self.syncing.set(was);
        let name = if root == Path::new("/") {
            "File System".to_string()
        } else if root == glib::home_dir() {
            "Home".to_string()
        } else {
            root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string())
        };
        let icon: gio::Icon = if root == Path::new("/") {
            gio::ThemedIcon::from_names(&["drive-harddisk", "folder"]).upcast()
        } else if root == glib::home_dir() {
            gio::ThemedIcon::from_names(&["user-home", "folder"]).upcast()
        } else {
            util::folder_icon()
        };
        let iter = self.store.insert_with_values(None, None, &[(T_ICON, &icon), (T_NAME, &name), (T_PATH, &root.to_string_lossy().to_string()), (T_LOADED, &false)]);
        self.add_placeholder(&iter);
        if let Some(p) = self.store.path(&iter) {
            self.load_children(&iter, None);
            self.tree.expand_row(&p, false);
        }
    }

    fn add_placeholder(&self, parent: &gtk::TreeIter) {
        self.store.insert_with_values(Some(parent), None, &[(T_NAME, &"…"), (T_PATH, &String::new()), (T_LOADED, &true)]);
    }

    /// Reads the subfolders of a node (once), then continues with `then`.
    fn load_children(&self, iter: &gtk::TreeIter, then: Option<Box<dyn FnOnce(&Sidebar)>>) {
        let loaded: bool = self.store.value(iter, T_LOADED as i32).get().unwrap_or(false);
        if loaded {
            if let Some(f) = then {
                f(self);
            }
            return;
        }
        self.store.set_value(iter, T_LOADED, &true.to_value());
        let path: String = self.store.value(iter, T_PATH as i32).get().unwrap_or_default();
        let show_hidden = self.window.upgrade().is_some_and(|w| w.app.settings.borrow().show_hidden);
        let Some(tree_path) = self.store.path(iter) else { return };
        let row = gtk::TreeRowReference::new(&self.store, &tree_path);
        let me = SIDEBARS.with(|s| s.borrow().iter().find(|(_, sb)| std::ptr::eq(&**sb, self)).map(|(_, sb)| Rc::downgrade(sb)));
        let Some(me) = me else { return };
        glib::spawn_future_local(async move {
            let dirs = gio::spawn_blocking(move || subfolders(Path::new(&path), show_hidden)).await.unwrap_or_default();
            let Some(sb) = me.upgrade() else { return };
            let Some(iter) = row.and_then(|r| r.path()).and_then(|p| sb.store.iter(&p)) else { return };
            // Add the real subfolders first, then drop the placeholder: a row that is left
            // without children for a moment gets collapsed by GTK, which made the first
            // click on an expander look like it did nothing.
            let mut old = Vec::new();
            let mut child = sb.store.iter_children(Some(&iter));
            while let Some(c) = child {
                old.push(c.clone());
                child = if sb.store.iter_next(&c) { Some(c) } else { None };
            }
            for d in dirs {
                let icon = super::util::icon_for_entry(&d.entry);
                let child = sb.store.insert_with_values(Some(&iter), None, &[(T_ICON, &icon), (T_NAME, &d.name), (T_PATH, &d.entry.path.to_string_lossy().to_string()), (T_LOADED, &!d.has_children)]);
                if d.has_children {
                    sb.add_placeholder(&child);
                }
            }
            for c in old {
                sb.store.remove(&c);
            }
            if let Some(f) = then {
                f(&sb);
            }
        });
    }

    /// Expands the tree down to `dir` and selects it.
    fn reveal(&self, dir: &Path) {
        let Some(root) = self.root.borrow().clone() else { return };
        let Some(root_iter) = self.store.iter_first() else { return };
        let Ok(rest) = dir.strip_prefix(&root) else {
            self.syncing.set(true);
            self.tree.selection().unselect_all();
            self.syncing.set(false);
            return;
        };
        let parts: Vec<String> = rest.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        self.reveal_step(root_iter, parts, 0);
    }

    fn reveal_step(&self, iter: gtk::TreeIter, parts: Vec<String>, i: usize) {
        if i == parts.len() {
            if let Some(p) = self.store.path(&iter) {
                self.syncing.set(true);
                self.tree.expand_to_path(&p);
                self.tree.selection().select_path(&p);
                self.tree.scroll_to_cell(Some(&p), None::<&gtk::TreeViewColumn>, false, 0.0, 0.0);
                self.syncing.set(false);
            }
            return;
        }
        let Some(path) = self.store.path(&iter) else { return };
        let row = gtk::TreeRowReference::new(&self.store, &path);
        self.load_children(
            &iter,
            Some(Box::new(move |sb: &Sidebar| {
                let Some(iter) = row.and_then(|r| r.path()).and_then(|p| sb.store.iter(&p)) else { return };
                let mut child = sb.store.iter_children(Some(&iter));
                while let Some(c) = child {
                    let name: String = sb.store.value(&c, T_NAME as i32).get().unwrap_or_default();
                    if name == parts[i] {
                        sb.reveal_step(c, parts, i + 1);
                        return;
                    }
                    child = if sb.store.iter_next(&c) { Some(c) } else { None };
                }
                // A hidden folder while hidden files are off: show just this one, since the
                // user is in it.
                let parent: String = sb.store.value(&iter, T_PATH as i32).get().unwrap_or_default();
                let full = Path::new(&parent).join(&parts[i]);
                if let Ok(entry) = failbrauwser::fs::FileEntry::stat(&full) {
                    if entry.is_dir_like() {
                        let icon = util::icon_for_entry(&entry);
                        let c = sb.store.insert_with_values(Some(&iter), None, &[(T_ICON, &icon), (T_NAME, &parts[i]), (T_PATH, &full.to_string_lossy().to_string()), (T_LOADED, &false)]);
                        sb.add_placeholder(&c);
                        sb.reveal_step(c, parts, i + 1);
                        return;
                    }
                }
                if let Some(p) = sb.store.path(&iter) {
                    sb.syncing.set(true);
                    sb.tree.expand_to_path(&p);
                    sb.tree.selection().select_path(&p);
                    sb.syncing.set(false);
                }
            })),
        );
    }
}

struct Subfolder {
    name: String,
    entry: failbrauwser::fs::FileEntry,
    has_children: bool,
}

/// The subfolders of `dir`, sorted, each with whether it has subfolders itself.
/// Runs on a worker thread.
fn subfolders(dir: &Path, show_hidden: bool) -> Vec<Subfolder> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<Subfolder> = rd
        .filter_map(Result::ok)
        .filter(|e| show_hidden || !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| failbrauwser::fs::FileEntry::stat(&e.path()).ok())
        .filter(|e| e.is_dir_like())
        .map(|e| Subfolder { name: e.display_name(), has_children: has_subfolders(&e.path, show_hidden), entry: e })
        .collect();
    v.sort_by(|a, b| failbrauwser::fs::format::natural_cmp(&a.name, &b.name));
    v
}

/// Cheap check whether a folder has any subfolder (stops at the first).
fn has_subfolders(dir: &Path, show_hidden: bool) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    rd.filter_map(Result::ok).take(2000).any(|e| {
        (show_hidden || !e.file_name().to_string_lossy().starts_with('.'))
            && e.file_type().map(|t| t.is_dir()).unwrap_or(false)
    })
}
