//! TortoiseGit's list of changed files, as in its Commit, Revert, Add, Resolve, Check for
//! Modifications and Log dialogs: a check box (where files are picked), the path with the
//! file's icon and overlay, extension, status, lines added and removed, the text colored by
//! the status.

use super::icons;
use failbrauwser::vcs::git::{Change, Status};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    /// The lists shown, for tests to find them by window and name.
    static LISTS: RefCell<Vec<std::rc::Weak<FileList>>> = const { RefCell::new(Vec::new()) };
}

/// The list named `name` in the shown window whose title contains `title` (tests).
pub fn find(title: &str, name: &str) -> Option<Rc<FileList>> {
    LISTS.with(|l| {
        l.borrow().iter().filter_map(|w| w.upgrade()).find(|f| {
            f.view.widget_name() == name
                && f.view.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok()).is_some_and(|t| t.is_visible() && t.title().is_some_and(|x| x.contains(title)))
        })
    })
}

const C_CHECK: u32 = 0;
const C_PATH: u32 = 1;
const C_EXT: u32 = 2;
const C_STATUS: u32 = 3;
const C_ADDED: u32 = 4;
const C_REMOVED: u32 = 5;
const C_ICON: u32 = 6;
const C_COLOR: u32 = 7;
const C_INDEX: u32 = 8;

pub struct FileList {
    pub scroll: gtk::ScrolledWindow,
    pub view: gtk::TreeView,
    store: gtk::ListStore,
    changes: RefCell<Vec<Change>>,
    checkable: bool,
    on_toggle: RefCell<Vec<Box<dyn Fn()>>>,
}

/// TortoiseGit's text colors for each status, darker on light themes and lighter on dark.
pub fn status_color(s: Status, dark: bool) -> Option<&'static str> {
    Some(match (s, dark) {
        (Status::Modified, false) => "#00008b",
        (Status::Modified, true) => "#8fb4ff",
        (Status::Added | Status::Renamed, false) => "#800080",
        (Status::Added | Status::Renamed, true) => "#e29ae2",
        (Status::Deleted, false) => "#8b0000",
        (Status::Deleted, true) => "#ff8f8f",
        (Status::Conflicted, false) => "#ff0000",
        (Status::Conflicted, true) => "#ff6060",
        (Status::Ignored, _) => "#808080",
        _ => return None,
    })
}

/// Whether the theme draws light text on dark backgrounds.
pub fn dark_theme(w: &impl IsA<gtk::Widget>) -> bool {
    let c = w.style_context().color(gtk::StateFlags::NORMAL);
    (c.red() + c.green() + c.blue()) / 3.0 > 0.5
}

impl FileList {
    pub fn new(name: &str, checkable: bool) -> Rc<FileList> {
        let store = gtk::ListStore::new(&[
            glib::Type::BOOL,
            glib::Type::STRING,
            glib::Type::STRING,
            glib::Type::STRING,
            glib::Type::STRING,
            glib::Type::STRING,
            gtk::gdk_pixbuf::Pixbuf::static_type(),
            glib::Type::STRING,
            glib::Type::U32,
        ]);
        let view = gtk::TreeView::with_model(&store);
        view.set_widget_name(name);
        view.selection().set_mode(gtk::SelectionMode::Multiple);
        view.set_enable_search(true);
        view.set_search_column(C_PATH as i32);
        let list = Rc::new(FileList {
            scroll: gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE),
            view: view.clone(),
            store: store.clone(),
            changes: RefCell::new(Vec::new()),
            checkable,
            on_toggle: RefCell::new(Vec::new()),
        });
        let path_col = gtk::TreeViewColumn::new();
        path_col.set_title("Path");
        path_col.set_resizable(true);
        path_col.set_expand(true);
        path_col.set_sort_column_id(C_PATH as i32);
        if checkable {
            let toggle = gtk::CellRendererToggle::new();
            TreeViewColumnExt::pack_start(&path_col, &toggle, false);
            TreeViewColumnExt::add_attribute(&path_col, &toggle, "active", C_CHECK as i32);
            let weak = Rc::downgrade(&list);
            toggle.connect_toggled(move |_, path| {
                let Some(l) = weak.upgrade() else { return };
                if let Some(it) = l.store.iter(&path) {
                    let v: bool = l.store.value(&it, C_CHECK as i32).get().unwrap_or(false);
                    l.store.set_value(&it, C_CHECK, &(!v).to_value());
                    l.toggled();
                }
            });
        }
        let pix = gtk::CellRendererPixbuf::new();
        TreeViewColumnExt::pack_start(&path_col, &pix, false);
        TreeViewColumnExt::add_attribute(&path_col, &pix, "pixbuf", C_ICON as i32);
        let text = gtk::CellRendererText::new();
        text.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        TreeViewColumnExt::pack_start(&path_col, &text, true);
        TreeViewColumnExt::add_attribute(&path_col, &text, "text", C_PATH as i32);
        TreeViewColumnExt::add_attribute(&path_col, &text, "foreground", C_COLOR as i32);
        view.append_column(&path_col);
        for (title, col, right) in [("Extension", C_EXT, false), ("Status", C_STATUS, false), ("Lines added", C_ADDED, true), ("Lines removed", C_REMOVED, true)] {
            let c = gtk::TreeViewColumn::new();
            c.set_title(title);
            c.set_resizable(true);
            c.set_sort_column_id(col as i32);
            let r = gtk::CellRendererText::new();
            if right {
                r.set_xalign(1.0);
            }
            TreeViewColumnExt::pack_start(&c, &r, true);
            TreeViewColumnExt::add_attribute(&c, &r, "text", col as i32);
            TreeViewColumnExt::add_attribute(&c, &r, "foreground", C_COLOR as i32);
            view.append_column(&c);
        }
        list.scroll.set_shadow_type(gtk::ShadowType::In);
        list.scroll.set_size_request(-1, 160);
        list.scroll.set_vexpand(true);
        list.scroll.add(&view);
        super::dialog::keep_alive(&list.view, list.clone());
        LISTS.with(|l| {
            let mut l = l.borrow_mut();
            l.retain(|w| w.strong_count() > 0);
            l.push(Rc::downgrade(&list));
        });
        list
    }

    fn toggled(&self) {
        for f in self.on_toggle.borrow().iter() {
            f();
        }
    }

    /// Called whenever a check box changes.
    pub fn connect_toggled(&self, f: impl Fn() + 'static) {
        self.on_toggle.borrow_mut().push(Box::new(f));
    }

    /// Shows `changes`; `checked` says which start checked.
    pub fn set(&self, changes: Vec<Change>, checked: impl Fn(&Change) -> bool) {
        let dark = dark_theme(&self.view);
        let kept: Vec<(String, bool)> = self.rows_checked();
        self.store.clear();
        for (i, c) in changes.iter().enumerate() {
            let it = self.store.append();
            let ct = gio::content_type_guess(Some(std::path::Path::new(&c.path)), None).0;
            let icon = super::super::util::icon_for_type(&ct);
            let pb = super::super::icons::pixbuf_for(&icon, 16);
            let shown = match &c.old_path {
                Some(old) => format!("{} (from {old})", c.path),
                None => c.path.clone(),
            };
            let n = |v: Option<u64>| v.map(|x| x.to_string()).unwrap_or_default();
            // A list shown again keeps what was ticked.
            let check = kept.iter().find(|(p, _)| *p == c.path).map(|(_, v)| *v).unwrap_or_else(|| checked(c));
            self.store.set(
                &it,
                &[
                    (C_CHECK, &check),
                    (C_PATH, &shown),
                    (C_EXT, &c.extension()),
                    (C_STATUS, &c.status.label()),
                    (C_ADDED, &n(c.added)),
                    (C_REMOVED, &n(c.removed)),
                    (C_ICON, &pb),
                    (C_COLOR, &status_color(c.status, dark)),
                    (C_INDEX, &(i as u32)),
                ],
            );
        }
        *self.changes.borrow_mut() = changes;
        self.toggled();
    }

    fn rows_checked(&self) -> Vec<(String, bool)> {
        let changes = self.changes.borrow();
        let mut out = Vec::new();
        self.store.foreach(|m, _, it| {
            let i: u32 = m.value(it, C_INDEX as i32).get().unwrap_or(0);
            if let Some(c) = changes.get(i as usize) {
                out.push((c.path.clone(), m.value(it, C_CHECK as i32).get().unwrap_or(false)));
            }
            false
        });
        out
    }

    /// The changes shown, in the list's order.
    pub fn all(&self) -> Vec<Change> {
        let changes = self.changes.borrow();
        let mut out = Vec::new();
        self.store.foreach(|m, _, it| {
            let i: u32 = m.value(it, C_INDEX as i32).get().unwrap_or(0);
            out.extend(changes.get(i as usize).cloned());
            false
        });
        out
    }

    /// The changes ticked.
    pub fn checked(&self) -> Vec<Change> {
        if !self.checkable {
            return self.all();
        }
        let changes = self.changes.borrow();
        let mut out = Vec::new();
        self.store.foreach(|m, _, it| {
            if m.value(it, C_CHECK as i32).get().unwrap_or(false) {
                let i: u32 = m.value(it, C_INDEX as i32).get().unwrap_or(0);
                out.extend(changes.get(i as usize).cloned());
            }
            false
        });
        out
    }

    /// The changes selected (highlighted).
    pub fn selected(&self) -> Vec<Change> {
        let (rows, model) = self.view.selection().selected_rows();
        let changes = self.changes.borrow();
        rows.iter()
            .filter_map(|p| model.iter(p))
            .filter_map(|it| model.value(&it, C_INDEX as i32).get::<u32>().ok())
            .filter_map(|i| changes.get(i as usize).cloned())
            .collect()
    }

    /// Ticks exactly the changes `f` picks.
    pub fn check_where(&self, f: impl Fn(&Change) -> bool) {
        let changes = self.changes.borrow().clone();
        self.store.foreach(|m, _, it| {
            let i: u32 = m.value(it, C_INDEX as i32).get().unwrap_or(0);
            if let Some(c) = changes.get(i as usize) {
                self.store.set_value(it, C_CHECK, &f(c).to_value());
            }
            false
        });
        self.toggled();
    }

    /// Ticks or unticks the change of `path`; false when there is none.
    pub fn set_checked(&self, path: &str, on: bool) -> bool {
        let changes = self.changes.borrow().clone();
        let mut found = false;
        self.store.foreach(|m, _, it| {
            let i: u32 = m.value(it, C_INDEX as i32).get().unwrap_or(0);
            if changes.get(i as usize).is_some_and(|c| c.path == path) {
                self.store.set_value(it, C_CHECK, &on.to_value());
                found = true;
            }
            false
        });
        self.toggled();
        found
    }

    /// The rows as tests compare them: "+path Status" (ticked) or "-path Status".
    pub fn describe(&self) -> Vec<String> {
        let checked: Vec<String> = self.checked().into_iter().map(|c| c.path).collect();
        self.all()
            .into_iter()
            .map(|c| {
                let mark = if !self.checkable { "" } else if checked.contains(&c.path) { "+" } else { "-" };
                format!("{mark}{} {}", c.path, c.status.label())
            })
            .collect()
    }

    /// Called with the change of a row double-clicked (TortoiseGit: shows its diff).
    pub fn connect_activated(self: &Rc<Self>, f: impl Fn(&Change) + 'static) {
        let weak = Rc::downgrade(self);
        self.view.connect_row_activated(move |_, path, _| {
            let Some(l) = weak.upgrade() else { return };
            let Some(it) = l.store.iter(path) else { return };
            let i: u32 = l.store.value(&it, C_INDEX as i32).get().unwrap_or(0);
            let c = l.changes.borrow().get(i as usize).cloned();
            if let Some(c) = c {
                f(&c);
            }
        });
    }

    /// Activates the row of `path` as a double click would (tests).
    pub fn activate(&self, path: &str) -> bool {
        let mut target = None;
        let changes = self.changes.borrow().clone();
        self.store.foreach(|m, p, it| {
            let i: u32 = m.value(it, C_INDEX as i32).get().unwrap_or(0);
            if changes.get(i as usize).is_some_and(|c| c.path == path) {
                target = Some(p.clone());
                return true;
            }
            false
        });
        match target {
            Some(p) => {
                self.view.selection().unselect_all();
                self.view.selection().select_path(&p);
                if let Some(col) = self.view.column(0) {
                    self.view.row_activated(&p, &col);
                }
                true
            }
            None => false,
        }
    }

    /// A right-click menu for the selected rows, made by `menu` when there are any.
    pub fn connect_menu(self: &Rc<Self>, menu: impl Fn(&[Change]) -> Option<gtk::Menu> + 'static) {
        let weak = Rc::downgrade(self);
        self.view.connect_button_press_event(move |view, ev| {
            if ev.button() != 3 {
                return glib::Propagation::Proceed;
            }
            let Some(l) = weak.upgrade() else { return glib::Propagation::Proceed };
            let (x, y) = ev.position();
            if let Some((Some(p), ..)) = view.path_at_pos(x as i32, y as i32) {
                if !view.selection().path_is_selected(&p) {
                    view.selection().unselect_all();
                    view.selection().select_path(&p);
                }
            }
            let sel = l.selected();
            if sel.is_empty() {
                return glib::Propagation::Stop;
            }
            if let Some(m) = menu(&sel) {
                m.set_attach_widget(Some(view));
                m.show_all();
                m.popup_at_pointer(Some(ev));
            }
            glib::Propagation::Stop
        });
    }
}

/// A menu item with one of the Git icons in front.
pub fn menu_item(label: &str, icon: &str) -> gtk::MenuItem {
    let item = gtk::MenuItem::new();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.pack_start(&icons::image(icon, 16), false, false, 0);
    let l = gtk::Label::new(Some(label));
    l.set_xalign(0.0);
    row.pack_start(&l, true, true, 0);
    item.add(&row);
    item
}
