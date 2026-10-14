//! One tab: a location shown as detailed list or icons, with its own history.

use super::backend;
use super::model::{self, Item};
use failbrauwser::config::{Settings, ViewMode};
use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::rc::{Rc, Weak};
use std::time::Duration;

/// What a pane tells its window.
pub enum PaneEvent {
    /// Location, history or loading state changed.
    Location,
    Selection,
    /// Rows were added, removed or updated.
    Contents,
    /// Right click on the view (items already selected); `None` position means keyboard.
    ContextMenu(Option<gdk::EventButton>),
    HeaderMenu(gdk::EventButton),
    OpenInNewTab(Location),
    /// A click into empty space (ends typing in the path bar).
    ClickedEmpty,
    /// Other rows came into view (thumbnails for them).
    Scrolled,
    /// Ctrl+scroll: zoom in (positive) or out.
    Zoom(i32),
    /// Files were activated that the pane does not open itself.
    OpenFiles(Vec<Item>),
    /// The archive shown is encrypted; ask for the password, then reload.
    PasswordNeeded(Location),
    /// A network share did not answer (its top folder).
    ShareNotResponding(std::path::PathBuf),
}

struct State {
    location: Location,
    back: Vec<Location>,
    forward: Vec<Location>,
    /// Everything read from the location, hidden files included.
    all: Vec<Item>,
    rows: HashMap<u64, (Item, gtk::TreeIter)>,
    by_name: HashMap<OsString, u64>,
    next_key: u64,
    generation: u64,
    monitor: Option<gio::FileMonitor>,
    refresh_scheduled: bool,
    /// A name to select once the load with this generation finishes.
    pending_select: Option<(u64, OsString)>,
    /// Names to select as soon as they show up (after create, paste, rename), and until when.
    select_when_present: Vec<OsString>,
    select_deadline: Option<std::time::Instant>,
    loading: bool,
    error: Option<String>,
}

pub struct Pane {
    pub root: gtk::Box,
    info: gtk::InfoBar,
    info_label: gtk::Label,
    pub stack: gtk::Stack,
    pub store: gtk::ListStore,
    pub tree: gtk::TreeView,
    pub icons: gtk::IconView,
    settings: Rc<RefCell<Settings>>,
    folders_first: Rc<Cell<bool>>,
    st: RefCell<State>,
    listeners: RefCell<Vec<Box<dyn Fn(&PaneEvent)>>>,
    weak: RefCell<Weak<Pane>>,
    /// Suppresses the sort-changed handler while the pane sets the sort itself.
    applying_sort: Cell<bool>,
    /// While > 0 the model is being changed in bulk: events are held back.
    quiet: Cell<u32>,
    selection_dirty: Cell<bool>,
    /// Icon sizes in pixels for the list and the icon view (zoom).
    list_px: Rc<Cell<i32>>,
    grid_px: Rc<Cell<i32>>,
    grid_pix: gtk::CellRendererPixbuf,
    grid_text: gtk::CellRendererText,
    /// Rows holding a thumbnail, oldest first (at most [`MAX_THUMBS`]).
    thumbs: RefCell<std::collections::VecDeque<OsString>>,
    scroll_pending: Cell<bool>,
    /// Filter-as-you-type for the current folder (empty: everything).
    filter: RefCell<failbrauwser::search::Query>,
    /// A search is running for this tab; cancelled when it goes elsewhere.
    search_cancel: RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>,
    searching: Cell<bool>,
}

/// Thumbnails kept in memory per tab; older ones are dropped (and come back from the disk
/// cache when scrolled into view again).
const MAX_THUMBS: usize = 400;

impl Pane {
    pub fn new(settings: Rc<RefCell<Settings>>, location: Location) -> Rc<Pane> {
        let store = model::new_store();
        let folders_first = Rc::new(Cell::new(settings.borrow().folders_first));
        model::install_sorting(&store, folders_first.clone());

        let tree = gtk::TreeView::with_model(&store);
        tree.set_headers_clickable(true);
        tree.set_rubber_banding(true);
        tree.set_enable_search(true);
        tree.set_search_column(model::COL_NAME as i32);
        tree.set_fixed_height_mode(false);
        tree.selection().set_mode(gtk::SelectionMode::Multiple);
        let tree_scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        tree_scroll.add(&tree);

        let icons = gtk::IconView::with_model(&store);
        icons.set_selection_mode(gtk::SelectionMode::Multiple);
        icons.set_item_width(112);
        icons.set_column_spacing(4);
        icons.set_row_spacing(4);
        let list_px = Rc::new(Cell::new(0));
        let grid_px = Rc::new(Cell::new(0));
        let pix = gtk::CellRendererPixbuf::new();
        icons.pack_start(&pix, false);
        super::icons::bind(&icons, &pix, grid_px.clone());
        let txt = gtk::CellRendererText::new();
        gtk::prelude::CellRendererExt::set_alignment(&txt, 0.5, 0.0);
        txt.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        txt.set_wrap_width(108);
        txt.set_xalign(0.5);
        txt.set_property("alignment", gtk::pango::Alignment::Center);
        icons.pack_start(&txt, false);
        icons.add_attribute(&txt, "text", model::COL_NAME as i32);
        let icon_scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        icon_scroll.add(&icons);

        let stack = gtk::Stack::new();
        stack.add_named(&tree_scroll, "list");
        stack.add_named(&icon_scroll, "icons");

        let info = gtk::InfoBar::new();
        info.set_message_type(gtk::MessageType::Error);
        let info_label = gtk::Label::new(None);
        info_label.set_line_wrap(true);
        info_label.set_xalign(0.0);
        info.content_area().add(&info_label);
        info.set_no_show_all(true);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.pack_start(&info, false, false, 0);
        root.pack_start(&stack, true, true, 0);
        root.show_all();

        let pane = Rc::new(Pane {
            root,
            info,
            info_label,
            stack,
            store,
            tree,
            icons,
            settings,
            folders_first,
            st: RefCell::new(State {
                location: location.clone(),
                back: Vec::new(),
                forward: Vec::new(),
                all: Vec::new(),
                rows: HashMap::new(),
                by_name: HashMap::new(),
                next_key: 1,
                generation: 0,
                monitor: None,
                refresh_scheduled: false,
                pending_select: None,
                select_when_present: Vec::new(),
                select_deadline: None,
                loading: false,
                error: None,
            }),
            listeners: RefCell::new(Vec::new()),
            weak: RefCell::new(Weak::new()),
            applying_sort: Cell::new(false),
            quiet: Cell::new(0),
            selection_dirty: Cell::new(false),
            list_px,
            grid_px,
            grid_pix: pix,
            grid_text: txt,
            thumbs: RefCell::new(std::collections::VecDeque::new()),
            scroll_pending: Cell::new(false),
            filter: RefCell::new(failbrauwser::search::Query::new("")),
            search_cancel: RefCell::new(None),
            searching: Cell::new(false),
        });
        *pane.weak.borrow_mut() = Rc::downgrade(&pane);
        pane.apply_zoom_sizes();
        pane.build_columns();
        pane.apply_view_mode();
        pane.apply_sort();
        pane.connect_signals();
        pane.load(location, Vec::new(), true);
        pane
    }

    fn me(&self) -> Rc<Pane> {
        self.weak.borrow().upgrade().expect("pane alive")
    }

    pub fn connect(&self, f: impl Fn(&PaneEvent) + 'static) {
        self.listeners.borrow_mut().push(Box::new(f));
    }

    pub fn emit(&self, ev: PaneEvent) {
        if self.quiet.get() > 0 {
            // Row changes emit selection signals mid-update; report them once afterwards.
            if matches!(ev, PaneEvent::Selection) {
                self.selection_dirty.set(true);
            }
            return;
        }
        for l in self.listeners.borrow().iter() {
            l(&ev);
        }
    }

    /// Runs a bulk model change with events held back.
    fn quietly<T>(&self, f: impl FnOnce() -> T) -> T {
        self.quiet.set(self.quiet.get() + 1);
        let r = f();
        self.quiet.set(self.quiet.get() - 1);
        if self.quiet.get() == 0 && self.selection_dirty.replace(false) {
            self.emit(PaneEvent::Selection);
        }
        r
    }

    // ── Accessors ────────────────────────────────────────────────────

    pub fn location(&self) -> Location {
        self.st.borrow().location.clone()
    }
    pub fn can_go_back(&self) -> bool {
        !self.st.borrow().back.is_empty()
    }
    pub fn can_go_forward(&self) -> bool {
        !self.st.borrow().forward.is_empty()
    }
    pub fn refresh_pending(&self) -> bool {
        self.st.borrow().refresh_scheduled
    }
    pub fn is_loading(&self) -> bool {
        self.st.borrow().loading
    }
    pub fn error(&self) -> Option<String> {
        self.st.borrow().error.clone()
    }
    pub fn item_count(&self) -> usize {
        self.st.borrow().rows.len()
    }
    pub fn has_name(&self, name: &str) -> bool {
        self.st.borrow().all.iter().any(|i| i.display_name() == name)
    }

    pub fn focus_view(&self) {
        if self.stack.visible_child_name().as_deref() == Some("icons") {
            self.icons.grab_focus();
        } else {
            self.tree.grab_focus();
        }
    }

    fn view_is_icons(&self) -> bool {
        self.stack.visible_child_name().as_deref() == Some("icons")
    }

    pub fn selected_keys(&self) -> Vec<u64> {
        // While rows are rebuilt the views are detached from the model.
        if self.tree.model().is_none() || self.icons.model().is_none() {
            return Vec::new();
        }
        let paths: Vec<gtk::TreePath> = if self.view_is_icons() {
            self.icons.selected_items()
        } else {
            self.tree.selection().selected_rows().0
        };
        paths
            .iter()
            .filter_map(|p| self.store.iter(p))
            .map(|it| self.store.value(&it, model::COL_KEY as i32).get::<u64>().unwrap_or_default())
            .collect()
    }

    pub fn selected_items(&self) -> Vec<Item> {
        let st = self.st.borrow();
        self.selected_keys().iter().filter_map(|k| st.rows.get(k).map(|(i, _)| i.clone())).collect()
    }

    pub fn select_all(&self) {
        if self.view_is_icons() {
            self.icons.select_all();
        } else {
            self.tree.selection().select_all();
        }
    }

    pub fn unselect_all(&self) {
        self.icons.unselect_all();
        self.tree.selection().unselect_all();
    }

    /// Selects the rows with these names and scrolls the first into view.
    pub fn select_names(&self, names: &[OsString]) {
        // An explicit selection wins over one still waiting for its rows.
        self.cancel_pending_selection();
        let paths: Vec<gtk::TreePath> = {
            let st = self.st.borrow();
            names
                .iter()
                .filter_map(|n| st.by_name.get(n).and_then(|k| st.rows.get(k)))
                .filter_map(|(_, iter)| self.store.path(iter))
                .collect()
        };
        self.unselect_all();
        // The keyboard cursor first: moving it resets the selection to that one row.
        if let Some(p) = paths.first() {
            if self.view_is_icons() {
                self.icons.set_cursor(p, None::<&gtk::CellRenderer>, false);
            } else {
                self.tree.set_cursor(p, None::<&gtk::TreeViewColumn>, false);
            }
        }
        for p in &paths {
            self.tree.selection().select_path(p);
            self.icons.select_path(p);
        }
        if let Some(p) = paths.first() {
            if self.view_is_icons() {
                self.icons.scroll_to_path(p, false, 0.0, 0.0);
            } else {
                self.tree.scroll_to_cell(Some(p), None::<&gtk::TreeViewColumn>, false, 0.0, 0.0);
            }
        }
    }

    // ── Navigation ───────────────────────────────────────────────────

    pub fn navigate(&self, loc: Location) {
        {
            let mut st = self.st.borrow_mut();
            if st.location == loc {
                drop(st);
                self.reload();
                return;
            }
            let old = std::mem::replace(&mut st.location, loc.clone());
            st.back.push(old);
            st.forward.clear();
        }
        self.load(loc, Vec::new(), true);
    }

    /// Goes up; the folder we came from ends up selected.
    pub fn go_up(&self) {
        let loc = self.location();
        if let Some(parent) = loc.parent() {
            let came_from = OsString::from(loc.title());
            self.navigate(parent);
            let mut st = self.st.borrow_mut();
            st.pending_select = Some((st.generation, came_from));
        }
    }

    pub fn go_back(&self) {
        let target = {
            let mut st = self.st.borrow_mut();
            let Some(prev) = st.back.pop() else { return };
            let cur = std::mem::replace(&mut st.location, prev.clone());
            st.forward.push(cur);
            prev
        };
        self.load(target, Vec::new(), true);
    }

    pub fn go_forward(&self) {
        let target = {
            let mut st = self.st.borrow_mut();
            let Some(next) = st.forward.pop() else { return };
            let cur = std::mem::replace(&mut st.location, next.clone());
            st.back.push(cur);
            next
        };
        self.load(target, Vec::new(), true);
    }

    /// Reads the location again, keeping the selection.
    pub fn reload(&self) {
        let keep: Vec<OsString> = self.selected_items().iter().map(Item::os_name).collect();
        let loc = self.location();
        self.load(loc, keep, false);
    }

    fn load(&self, loc: Location, select: Vec<OsString>, fresh: bool) {
        if let Some(c) = self.search_cancel.borrow_mut().take() {
            c.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.searching.set(false);
        // The Folder column exists only in search results.
        let is_search = matches!(loc, Location::Search(_));
        let has_folder_col = self.tree.columns().iter().any(|c| c.title().as_deref() == Some("Folder"));
        if is_search != has_folder_col {
            self.build_columns_for(is_search);
        }
        if let Location::Search(s) = &loc {
            self.start_search(s.clone());
            return;
        }
        // A network share is asked first, off the UI thread: one that stopped answering
        // would otherwise hang everything that looks at the folder.
        if let Some(dir) = loc.local_path().filter(|p| failbrauwser::ops::gvfs::is_gvfs_path(p)).map(|p| p.to_path_buf()) {
            let me = self.me();
            self.st.borrow_mut().loading = true;
            glib::spawn_future_local(async move {
                let res = gio::spawn_blocking(move || failbrauwser::ops::gvfs::check(&dir, failbrauwser::ops::gvfs::TIMEOUT)).await;
                if me.location() != loc {
                    return;
                }
                match res {
                    Ok(Err(e)) if failbrauwser::ops::gvfs::not_responding_share(&e).is_some() => {
                        {
                            let mut st = me.st.borrow_mut();
                            st.generation += 1;
                            st.loading = false;
                            st.all.clear();
                            st.error = Some(e.to_string());
                        }
                        me.clear_rows();
                        me.show_error();
                        me.emit(PaneEvent::Location);
                        me.emit(PaneEvent::Contents);
                        if let Some(root) = failbrauwser::ops::gvfs::not_responding_share(&e) {
                            me.emit(PaneEvent::ShareNotResponding(root));
                        }
                    }
                    // Answered (maybe with an error the listing reports as usual).
                    _ => me.load_checked(loc, select, fresh),
                }
            });
            return;
        }
        self.load_checked(loc, select, fresh);
    }

    fn load_checked(&self, loc: Location, select: Vec<OsString>, fresh: bool) {
        let generation = {
            let mut st = self.st.borrow_mut();
            st.generation += 1;
            st.loading = true;
            st.error = None;
            st.monitor = None;
            st.refresh_scheduled = false;
            if fresh {
                st.all.clear();
            }
            st.generation
        };
        if fresh {
            self.clear_rows();
        }
        self.emit(PaneEvent::Location);
        let me = self.me();
        glib::spawn_future_local(async move {
            let l2 = loc.clone();
            let res = gio::spawn_blocking(move || backend::read_location(&l2)).await;
            if me.st.borrow().generation != generation {
                return;
            }
            let mut select = select;
            if let Some((g, name)) = me.st.borrow_mut().pending_select.take() {
                if g == generation {
                    select.push(name);
                }
            }
            match res {
                Ok(Ok(items)) => {
                    me.st.borrow_mut().all = items;
                    me.rebuild_rows(&select, fresh);
                    me.watch(&loc);
                }
                Ok(Err(err)) => {
                    me.st.borrow_mut().all.clear();
                    me.clear_rows();
                    me.st.borrow_mut().error = Some(err.message);
                    if err.needs_password {
                        me.emit(PaneEvent::PasswordNeeded(loc.clone()));
                    }
                }
                Err(_) => {
                    me.st.borrow_mut().error = Some("Reading the folder failed unexpectedly.".into());
                }
            }
            me.st.borrow_mut().loading = false;
            me.show_error();
            me.emit(PaneEvent::Location);
            me.emit(PaneEvent::Contents);
        });
    }

    fn show_error(&self) {
        match self.error() {
            Some(msg) => {
                self.info_label.set_text(&msg);
                self.info.show();
                self.info_label.show();
            }
            None => self.info.hide(),
        }
    }

    fn clear_rows(&self) {
        self.quietly(|| self.clear_rows_inner());
    }

    fn clear_rows_inner(&self) {
        let mut st = self.st.borrow_mut();
        st.rows.clear();
        st.by_name.clear();
        drop(st);
        self.store.clear();
    }

    /// Replaces all rows with the visible part of `all`.
    fn rebuild_rows(&self, select: &[OsString], fresh: bool) {
        self.quietly(|| self.rebuild_rows_inner(select, fresh));
    }

    fn rebuild_rows_inner(&self, select: &[OsString], fresh: bool) {
        // New rows have no thumbnails yet: ask again for what comes into view.
        super::thumbnails::reset(&self.me());
        let show_hidden = self.settings.borrow().show_hidden;
        let scroll = if fresh { None } else { self.tree.vadjustment().map(|a| a.value()) };
        // Detached views do not react to every single insert.
        self.tree.set_model(None::<&gtk::ListStore>);
        self.icons.set_model(None::<&gtk::ListStore>);
        self.clear_rows();
        let now = glib::real_time() / 1_000_000;
        {
            let mut st = self.st.borrow_mut();
            let filter = self.filter.borrow();
            let visible: Vec<Item> = st.all.iter().filter(|i| (show_hidden || !i.is_hidden()) && filter.matches(&i.display_name())).cloned().collect();
            let names: Vec<String> = visible.iter().map(Item::display_name).collect();
            let ranks = model::ranks(names.iter().map(String::as_str));
            for (item, rank) in visible.into_iter().zip(ranks) {
                let key = st.next_key;
                st.next_key += 1;
                let iter = self.store.append();
                model::fill_row(&self.store, &iter, key, &item, rank, now);
                st.by_name.insert(item.os_name(), key);
                st.rows.insert(key, (item, iter));
            }
        }
        self.thumbs.borrow_mut().clear();
        self.tree.set_model(Some(&self.store));
        self.icons.set_model(Some(&self.store));
        // An empty list must not cancel a selection waiting for these rows.
        if !select.is_empty() {
            self.select_names(select);
        }
        self.mark_cut();
        self.resolve_pending_selection();
        if let (Some(v), Some(adj)) = (scroll, self.tree.vadjustment()) {
            adj.set_value(v);
        } else if select.is_empty() {
            if let Some(adj) = self.tree.vadjustment() {
                adj.set_value(0.0);
            }
        }
    }

    /// Re-applies the hidden-files filter without reading again.
    pub fn refilter(&self) {
        let keep: Vec<OsString> = self.selected_items().iter().map(Item::os_name).collect();
        self.rebuild_rows(&keep, false);
        self.emit(PaneEvent::Contents);
    }

    // ── Live updates ─────────────────────────────────────────────────

    fn watch(&self, loc: &Location) {
        let Some(path) = backend::watch_path(loc) else { return };
        let file = gio::File::for_path(&path);
        let Ok(monitor) = file.monitor(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else {
            return;
        };
        let weak = self.weak.borrow().clone();
        monitor.connect_changed(move |_, _, _, ev| {
            if matches!(ev, gio::FileMonitorEvent::ChangesDoneHint | gio::FileMonitorEvent::PreUnmount) {
                return;
            }
            if let Some(p) = weak.upgrade() {
                p.schedule_refresh();
            }
        });
        self.st.borrow_mut().monitor = Some(monitor);
    }

    /// Coalesces bursts of change events into one re-read.
    pub fn schedule_refresh(&self) {
        {
            let mut st = self.st.borrow_mut();
            if st.refresh_scheduled {
                return;
            }
            st.refresh_scheduled = true;
        }
        let weak = self.weak.borrow().clone();
        glib::timeout_add_local_once(Duration::from_millis(250), move || {
            if let Some(p) = weak.upgrade() {
                p.refresh_incremental();
            }
        });
    }

    fn refresh_incremental(&self) {
        let (loc, generation) = {
            let mut st = self.st.borrow_mut();
            st.refresh_scheduled = false;
            (st.location.clone(), st.generation)
        };
        let me = self.me();
        glib::spawn_future_local(async move {
            let l2 = loc.clone();
            let Ok(res) = gio::spawn_blocking(move || backend::read_location(&l2)).await else { return };
            if me.st.borrow().generation != generation {
                return;
            }
            match res {
                Ok(items) => me.apply_update(items),
                Err(err) => {
                    // The folder itself went away: climb to the nearest existing parent.
                    me.st.borrow_mut().error = Some(err.message);
                    me.show_error();
                    let gone = match &loc {
                        Location::Dir(p) => Some(p.clone()),
                        Location::Archive(a) if !a.file.is_file() => a.file.parent().map(|p| p.to_path_buf()),
                        _ => None,
                    };
                    if let Some(mut up) = gone {
                        while !up.is_dir() && up.pop() {}
                        me.navigate(Location::Dir(up));
                    }
                }
            }
        });
    }

    /// Applies a new reading row by row, so selection and scrolling stay where they are.
    fn apply_update(&self, items: Vec<Item>) {
        let changed = self.quietly(|| self.apply_update_inner(items));
        self.show_error();
        if changed {
            self.mark_cut();
            self.resolve_pending_selection();
            self.emit(PaneEvent::Contents);
        }
    }

    /// Returns whether anything visible changed.
    fn apply_update_inner(&self, items: Vec<Item>) -> bool {
        let show_hidden = self.settings.borrow().show_hidden;
        let now = glib::real_time() / 1_000_000;
        let mut changed_any = false;
        {
            let mut st = self.st.borrow_mut();
            let new_names: HashSet<OsString> = items.iter().map(Item::os_name).collect();
            let gone: Vec<OsString> = st.by_name.keys().filter(|n| !new_names.contains(*n)).cloned().collect();
            for name in gone {
                if let Some(key) = st.by_name.remove(&name) {
                    if let Some((_, iter)) = st.rows.remove(&key) {
                        self.store.remove(&iter);
                        changed_any = true;
                    }
                }
            }
            let mut added = false;
            let filter = self.filter.borrow().clone();
            for item in items.iter().filter(|i| (show_hidden || !i.is_hidden()) && filter.matches(&i.display_name())) {
                let name = item.os_name();
                match st.by_name.get(&name).copied() {
                    Some(key) => {
                        let (old, iter) = st.rows.get(&key).cloned().unwrap();
                        if !old.same_as(item) {
                            let rank: u32 = self.store.value(&iter, model::COL_RANK as i32).get().unwrap_or_default();
                            model::fill_row(&self.store, &iter, key, item, rank, now);
                            st.rows.insert(key, (item.clone(), iter));
                            changed_any = true;
                        }
                    }
                    None => {
                        let key = st.next_key;
                        st.next_key += 1;
                        let iter = self.store.append();
                        model::fill_row(&self.store, &iter, key, item, 0, now);
                        st.by_name.insert(name, key);
                        st.rows.insert(key, (item.clone(), iter));
                        added = true;
                        changed_any = true;
                    }
                }
            }
            if added {
                // New names need their place in the natural order.
                let rows: Vec<(String, gtk::TreeIter)> =
                    st.rows.values().map(|(i, it)| (i.display_name(), it.clone())).collect();
                let ranks = model::ranks(rows.iter().map(|(n, _)| n.as_str()));
                for ((_, iter), rank) in rows.iter().zip(ranks) {
                    self.store.set_value(iter, model::COL_RANK, &rank.to_value());
                }
            }
            st.all = items;
            if st.error.take().is_some() {
                changed_any = true;
            }
        }
        changed_any
    }

    /// Row keys for displayed names (search results are keyed by path) (tests).
    pub fn keys_for_names(&self, names: &[OsString]) -> Vec<OsString> {
        let st = self.st.borrow();
        names
            .iter()
            .map(|n| {
                if st.by_name.contains_key(n) {
                    return n.clone();
                }
                st.rows.values().map(|(i, _)| i).find(|i| i.os_name() != *n && OsString::from(i.display_name()) == *n).map(|i| i.os_name()).unwrap_or_else(|| n.clone())
            })
            .collect()
    }

    pub fn is_searching(&self) -> bool {
        self.searching.get()
    }

    /// Shows only the rows whose names match (empty text: all rows).
    pub fn set_filter(&self, text: &str) {
        let q = failbrauwser::search::Query::new(text);
        let same = self.filter.borrow().is_empty() && q.is_empty();
        *self.filter.borrow_mut() = q;
        if !same {
            self.refilter();
        }
    }

    /// Goes somewhere without a new history entry (search text being typed).
    pub fn replace_location(&self, loc: Location) {
        self.st.borrow_mut().location = loc.clone();
        self.load(loc, Vec::new(), true);
    }

    /// Runs a search on a worker thread; results are added as they are found.
    fn start_search(&self, s: failbrauwser::location::SearchLoc) {
        let generation = {
            let mut st = self.st.borrow_mut();
            st.generation += 1;
            st.loading = false;
            st.error = None;
            st.monitor = None;
            st.all.clear();
            st.generation
        };
        self.clear_rows();
        self.searching.set(true);
        self.emit(PaneEvent::Location);
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        *self.search_cancel.borrow_mut() = Some(cancel.clone());
        let show_hidden = self.settings.borrow().show_hidden;
        let (tx, rx) = async_channel::unbounded::<Option<Vec<Item>>>();
        let root = s.root.clone();
        std::thread::Builder::new()
            .name("fb-search".into())
            .spawn(move || {
                use failbrauwser::search::{Hit, Options, Query, search};
                let q = Query::new(&s.query);
                let helper = s.archives.then(failbrauwser::archive::Helper::global);
                let rel = |p: &std::path::Path| -> String {
                    let parent = p.parent().unwrap_or(p);
                    match parent.strip_prefix(&root) {
                        Ok(r) if r.as_os_str().is_empty() => ".".into(),
                        Ok(r) => r.display().to_string(),
                        Err(_) => parent.display().to_string(),
                    }
                };
                let mut batch = Vec::new();
                let mut last = std::time::Instant::now();
                let opts = Options { hidden: show_hidden, archives: s.archives };
                search(&root, &q, opts, helper, &cancel, |hit| {
                    let item = match hit {
                        Hit::File(p) => failbrauwser::fs::FileEntry::stat(&p).ok().map(|e| Item::Found(e, rel(&p))),
                        Hit::InArchive { archive, entry, is_dir, size, mtime } => {
                            let inner = entry.rfind('/').map(|i| entry[..i].to_string()).unwrap_or_default();
                            let loc = failbrauwser::location::ArchiveLoc::root(archive.clone()).with_inner(&inner);
                            let name = entry.rsplit('/').next().unwrap_or(&entry).to_string();
                            let node = failbrauwser::archive::ArchiveNode { path: entry.clone(), name, is_dir, size, compressed: size, mtime, encrypted: false, implied: false };
                            let folder = format!("{}/{}", rel(&archive.join("x")), archive.file_name().unwrap_or_default().to_string_lossy());
                            let folder = if inner.is_empty() { folder } else { format!("{folder}/{inner}") };
                            Some(Item::FoundInArchive(loc, node, folder.trim_start_matches("./").to_string()))
                        }
                    };
                    batch.extend(item);
                    if batch.len() >= 200 || last.elapsed() >= std::time::Duration::from_millis(150) {
                        let _ = tx.send_blocking(Some(std::mem::take(&mut batch)));
                        last = std::time::Instant::now();
                    }
                });
                if !batch.is_empty() {
                    let _ = tx.send_blocking(Some(batch));
                }
                let _ = tx.send_blocking(None);
            })
            .expect("spawn search thread");
        let me = self.me();
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                if me.generation() != generation {
                    return;
                }
                match msg {
                    Some(items) => me.append_items(items),
                    None => {
                        me.searching.set(false);
                        me.emit(PaneEvent::Location);
                        me.emit(PaneEvent::Contents);
                        return;
                    }
                }
            }
        });
    }

    /// Adds rows (search results as they arrive).
    fn append_items(&self, items: Vec<Item>) {
        let now = glib::real_time() / 1_000_000;
        self.quietly(|| {
            let mut st = self.st.borrow_mut();
            for item in items {
                let key = st.next_key;
                st.next_key += 1;
                let iter = self.store.append();
                model::fill_row(&self.store, &iter, key, &item, 0, now);
                st.by_name.insert(item.os_name(), key);
                st.all.push(item.clone());
                st.rows.insert(key, (item, iter));
            }
            let rows: Vec<(String, gtk::TreeIter)> = st.rows.values().map(|(i, it)| (i.display_name(), it.clone())).collect();
            let ranks = model::ranks(rows.iter().map(|(n, _)| n.as_str()));
            for ((_, iter), rank) in rows.iter().zip(ranks) {
                self.store.set_value(iter, model::COL_RANK, &rank.to_value());
            }
        });
        self.emit(PaneEvent::Contents);
    }

    /// Selects `names` now if they are shown, otherwise once they appear (for a few
    /// seconds, and only until the user selects something else).
    pub fn select_when_present(&self, names: Vec<OsString>) {
        {
            let mut st = self.st.borrow_mut();
            st.select_when_present = names;
            st.select_deadline = Some(std::time::Instant::now() + Duration::from_secs(5));
        }
        self.resolve_pending_selection();
    }

    pub fn cancel_pending_selection(&self) {
        let mut st = self.st.borrow_mut();
        st.select_when_present.clear();
        st.select_deadline = None;
    }

    fn resolve_pending_selection(&self) {
        let names = {
            let st = self.st.borrow();
            if st.select_deadline.is_some_and(|d| std::time::Instant::now() > d) {
                drop(st);
                self.cancel_pending_selection();
                return;
            }
            if st.select_when_present.is_empty() || !st.select_when_present.iter().any(|n| st.by_name.contains_key(n)) {
                return;
            }
            st.select_when_present.clone()
        };
        self.select_names(&names);
        self.focus_view();
    }

    /// Dims the rows of files that are cut to the clipboard.
    pub fn mark_cut(&self) {
        let cut: HashSet<std::path::PathBuf> = super::clipboard::cut_paths().into_iter().collect();
        let st = self.st.borrow();
        for (item, iter) in st.rows.values() {
            let dim = item.is_hidden() || item.path().is_some_and(|p| cut.contains(p));
            let current: bool = self.store.value(iter, model::COL_SENSITIVE as i32).get().unwrap_or(true);
            if current == dim {
                self.store.set_value(iter, model::COL_SENSITIVE, &(!dim).to_value());
            }
        }
    }

    // ── Views ────────────────────────────────────────────────────────

    pub fn apply_view_mode(&self) {
        let mode = self.settings.borrow().view_mode;
        let focus = self.tree.has_focus() || self.icons.has_focus();
        let keys = self.selected_keys();
        // The Drives page has no list or icon view; it stays until the location changes.
        if self.location() != Location::Drives {
            self.stack.set_visible_child_name(if mode == ViewMode::Icons { "icons" } else { "list" });
        }
        // Carry the selection over to the other view.
        let st = self.st.borrow();
        for k in keys {
            if let Some((_, iter)) = st.rows.get(&k) {
                self.tree.selection().select_iter(iter);
                if let Some(p) = self.store.path(iter) {
                    self.icons.select_path(&p);
                }
            }
        }
        drop(st);
        if focus {
            self.focus_view();
        }
    }

    fn apply_zoom_sizes(&self) {
        let (list, grid) = {
            let s = self.settings.borrow();
            (failbrauwser::config::LIST_ZOOM[s.zoom_list], failbrauwser::config::ICON_ZOOM[s.zoom_icons])
        };
        self.list_px.set(list);
        self.grid_px.set(grid);
        self.grid_pix.set_fixed_size(grid + 8, grid + 8);
        // Room for the name under the icon grows with the icon.
        let item = (grid + 40).max(112);
        self.icons.set_item_width(item);
        self.grid_text.set_wrap_width(item - 4);
    }

    /// Redraws both views at the zoom levels from the settings.
    pub fn apply_zoom(&self) {
        self.apply_zoom_sizes();
        // Row heights are cached per column: new columns measure again.
        self.build_columns();
        self.icons.queue_resize();
    }

    /// Shows a thumbnail for the row `name`.
    pub fn set_thumbnail(&self, name: &OsString, pb: gtk::gdk_pixbuf::Pixbuf) {
        let st = self.st.borrow();
        let Some((_, iter)) = st.by_name.get(name).and_then(|k| st.rows.get(k)) else { return };
        self.store.set_value(iter, model::COL_THUMB, &Some(pb).to_value());
        let mut thumbs = self.thumbs.borrow_mut();
        thumbs.retain(|n| n != name);
        thumbs.push_back(name.clone());
        while thumbs.len() > MAX_THUMBS {
            if let Some(old) = thumbs.pop_front() {
                if let Some((_, it)) = st.by_name.get(&old).and_then(|k| st.rows.get(k)) {
                    self.store.set_value(it, model::COL_THUMB, &None::<gtk::gdk_pixbuf::Pixbuf>.to_value());
                }
            }
        }
    }

    /// Rows showing a thumbnail right now (tests).
    pub fn thumbnail_count(&self) -> usize {
        let mut n = 0;
        self.store.foreach(|m, _, it| {
            if m.value(it, model::COL_THUMB as i32).get::<Option<gtk::gdk_pixbuf::Pixbuf>>().ok().flatten().is_some() {
                n += 1;
            }
            false
        });
        n
    }

    /// Files in view that might get a thumbnail: (name, path, mtime, MIME type).
    pub fn visible_thumbnail_candidates(&self) -> Vec<(OsString, std::path::PathBuf, i64, String)> {
        let range = if self.view_is_icons() { self.icons.visible_range() } else { self.tree.visible_range() };
        let Some((start, end)) = range else { return Vec::new() };
        let st = self.st.borrow();
        let mut out = Vec::new();
        let Some(iter) = self.store.iter(&start) else { return out };
        // A few rows beyond the edge, so short scrolls find thumbnails ready.
        let last = end.indices().first().copied().unwrap_or(0) + 8;
        loop {
            let key: u64 = self.store.value(&iter, model::COL_KEY as i32).get().unwrap_or_default();
            let has: bool = self.store.value(&iter, model::COL_THUMB as i32).get::<Option<gtk::gdk_pixbuf::Pixbuf>>().ok().flatten().is_some();
            if let Some((item, _)) = st.rows.get(&key) {
                if let (false, Item::Fs(e) | Item::Trash(_, e) | Item::Found(e, _)) = (has, item) {
                    if !e.is_dir_like() {
                        out.push((item.os_name(), e.path.clone(), e.mtime, e.content_type.clone()));
                    }
                }
            }
            let idx = self.store.path(&iter).and_then(|p| p.indices().first().copied()).unwrap_or(i32::MAX);
            if idx >= last || !self.store.iter_next(&iter) {
                break;
            }
        }
        out
    }

    /// Current icon size of the list, in pixels (tests).
    pub fn list_icon_size(&self) -> i32 {
        self.list_px.get()
    }

    pub fn grid_icon_size(&self) -> i32 {
        self.grid_px.get()
    }

    pub fn apply_folders_first(&self) {
        self.folders_first.set(self.settings.borrow().folders_first);
        // Re-sorting needs the sort id to change; flip through unsorted and back.
        self.apply_sort();
    }

    pub fn apply_sort(&self) {
        let (id, desc) = {
            let s = self.settings.borrow();
            (model::sort_id_for(&s.sort_column), s.sort_descending)
        };
        self.applying_sort.set(true);
        self.store.set_unsorted();
        self.store.set_sort_column_id(
            gtk::SortColumn::Index(id),
            if desc { gtk::SortType::Descending } else { gtk::SortType::Ascending },
        );
        self.applying_sort.set(false);
    }

    /// (Re)creates the list columns from the settings.
    pub fn build_columns(&self) {
        let is_search = matches!(self.location(), Location::Search(_));
        self.build_columns_for(is_search);
    }

    fn build_columns_for(&self, search: bool) {
        for c in self.tree.columns() {
            self.tree.remove_column(&c);
        }
        let cols = self.settings.borrow().columns.clone();
        for id in cols {
            let col = gtk::TreeViewColumn::new();
            col.set_resizable(true);
            col.set_reorderable(false);
            col.set_sort_column_id(model::sort_id_for(&id) as i32);
            let text = gtk::CellRendererText::new();
            text.set_padding(4, 1);
            match id.as_str() {
                "name" => {
                    col.set_title("Name");
                    col.set_expand(true);
                    col.set_min_width(200);
                    let pix = gtk::CellRendererPixbuf::new();
                    let px = self.list_px.get();
                    pix.set_fixed_size(px + 4, px + 2);
                    TreeViewColumnExt::pack_start(&col, &pix, false);
                    super::icons::bind(&col, &pix, self.list_px.clone());
                    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    TreeViewColumnExt::pack_start(&col, &text, true);
                    TreeViewColumnExt::add_attribute(&col, &text, "text", model::COL_NAME as i32);
                }
                "size" | "dirsize" => {
                    col.set_title(if id == "size" { "Size" } else { "Total Size" });
                    text.set_xalign(1.0);
                    col.set_alignment(1.0);
                    TreeViewColumnExt::pack_start(&col, &text, true);
                    let src = if id == "size" { model::COL_SIZE_TEXT } else { model::COL_DIRSIZE_TEXT };
                    TreeViewColumnExt::add_attribute(&col, &text, "text", src as i32);
                    col.set_min_width(80);
                }
                "type" => {
                    col.set_title("Type");
                    TreeViewColumnExt::pack_start(&col, &text, true);
                    TreeViewColumnExt::add_attribute(&col, &text, "text", model::COL_TYPE as i32);
                    col.set_min_width(100);
                }
                "modified" => {
                    col.set_title("Date Modified");
                    TreeViewColumnExt::pack_start(&col, &text, true);
                    TreeViewColumnExt::add_attribute(&col, &text, "text", model::COL_MTIME_TEXT as i32);
                    col.set_min_width(120);
                }
                "permissions" => {
                    col.set_title("Permissions");
                    text.set_property("family", "monospace");
                    TreeViewColumnExt::pack_start(&col, &text, true);
                    TreeViewColumnExt::add_attribute(&col, &text, "text", model::COL_PERMS as i32);
                }
                "owner" => {
                    col.set_title("Owner");
                    TreeViewColumnExt::pack_start(&col, &text, true);
                    TreeViewColumnExt::add_attribute(&col, &text, "text", model::COL_OWNER as i32);
                }
                "group" => {
                    col.set_title("Group");
                    TreeViewColumnExt::pack_start(&col, &text, true);
                    TreeViewColumnExt::add_attribute(&col, &text, "text", model::COL_GROUP as i32);
                }
                _ => continue,
            }
            TreeViewColumnExt::add_attribute(&col, &text, "sensitive", model::COL_SENSITIVE as i32);
            self.tree.append_column(&col);
            // Right click on any header offers the column choice.
            if let Some(button) = col.button() {
                let weak = self.weak.borrow().clone();
                button.connect_button_press_event(move |_, ev| {
                    if ev.button() == 3 {
                        if let Some(p) = weak.upgrade() {
                            p.emit(PaneEvent::HeaderMenu(ev.clone()));
                        }
                        return glib::Propagation::Stop;
                    }
                    glib::Propagation::Proceed
                });
            }
        }
        if search {
            // Where each result is, right after its name.
            let col = gtk::TreeViewColumn::new();
            col.set_title("Folder");
            col.set_resizable(true);
            col.set_min_width(160);
            let text = gtk::CellRendererText::new();
            text.set_ellipsize(gtk::pango::EllipsizeMode::Start);
            text.set_padding(4, 1);
            TreeViewColumnExt::pack_start(&col, &text, true);
            TreeViewColumnExt::add_attribute(&col, &text, "text", model::COL_FOLDER as i32);
            self.tree.insert_column(&col, 1);
        }
    }

    // ── Signals ──────────────────────────────────────────────────────

    fn connect_signals(&self) {
        let weak = self.weak.borrow().clone();
        self.tree.connect_row_activated(move |_, path, _| {
            if let Some(p) = weak.upgrade() {
                p.activate_paths(std::slice::from_ref(path));
            }
        });
        let weak = self.weak.borrow().clone();
        self.icons.connect_item_activated(move |_, path| {
            if let Some(p) = weak.upgrade() {
                p.activate_paths(std::slice::from_ref(path));
            }
        });
        let weak = self.weak.borrow().clone();
        self.tree.selection().connect_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.emit(PaneEvent::Selection);
            }
        });
        let weak = self.weak.borrow().clone();
        self.icons.connect_selection_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.emit(PaneEvent::Selection);
            }
        });
        let weak = self.weak.borrow().clone();
        self.store.connect_sort_column_changed(move |store| {
            let Some(p) = weak.upgrade() else { return };
            if p.applying_sort.get() {
                return;
            }
            if let Some((gtk::SortColumn::Index(i), order)) = store.sort_column_id() {
                if let Some(id) = failbrauwser::config::COLUMN_IDS.get(i as usize) {
                    let mut s = p.settings.borrow_mut();
                    s.sort_column = id.to_string();
                    s.sort_descending = order == gtk::SortType::Descending;
                }
            }
        });

        // Mouse: right click menus, middle click opens folders in a tab, back/forward buttons.
        let weak = self.weak.borrow().clone();
        self.tree.connect_button_press_event(move |tree, ev| {
            let Some(p) = weak.upgrade() else { return glib::Propagation::Proceed };
            let (x, y) = ev.position();
            let hit = tree.path_at_pos(x as i32, y as i32).and_then(|(path, ..)| path);
            p.handle_button(ev, hit, |path| tree.selection().path_is_selected(path), |path| {
                tree.selection().unselect_all();
                tree.selection().select_path(path);
                tree.set_cursor(path, None::<&gtk::TreeViewColumn>, false);
            })
        });
        let weak = self.weak.borrow().clone();
        self.icons.connect_button_press_event(move |icons, ev| {
            let Some(p) = weak.upgrade() else { return glib::Propagation::Proceed };
            let (x, y) = ev.position();
            let hit = icons.path_at_pos(x as i32, y as i32);
            p.handle_button(ev, hit, |path| icons.path_is_selected(path), |path| {
                icons.unselect_all();
                icons.select_path(path);
                icons.set_cursor(path, None::<&gtk::CellRenderer>, false);
            })
        });
        // Rows coming into view may want thumbnails: after scrolling, and after layout
        // (the scroll range changes once rows are measured). Settles before asking.
        for adj in [self.tree.vadjustment(), self.icons.vadjustment()].into_iter().flatten() {
            let notify = {
                let weak = self.weak.borrow().clone();
                move || {
                    let Some(p) = weak.upgrade() else { return };
                    if p.scroll_pending.replace(true) {
                        return;
                    }
                    let weak = Rc::downgrade(&p);
                    glib::timeout_add_local_once(Duration::from_millis(120), move || {
                        if let Some(p) = weak.upgrade() {
                            p.scroll_pending.set(false);
                            p.emit(PaneEvent::Scrolled);
                        }
                    });
                }
            };
            let n2 = notify.clone();
            adj.connect_value_changed(move |_| n2());
            adj.connect_changed(move |_| notify());
        }
        // Ctrl + mouse wheel zooms.
        for w in [self.tree.upcast_ref::<gtk::Widget>(), self.icons.upcast_ref()] {
            let weak = self.weak.borrow().clone();
            w.add_events(gdk::EventMask::SCROLL_MASK | gdk::EventMask::SMOOTH_SCROLL_MASK);
            w.connect_scroll_event(move |_, ev| {
                if !ev.state().contains(gdk::ModifierType::CONTROL_MASK) {
                    return glib::Propagation::Proceed;
                }
                let Some(p) = weak.upgrade() else { return glib::Propagation::Proceed };
                let step = match ev.direction() {
                    gdk::ScrollDirection::Up => 1,
                    gdk::ScrollDirection::Down => -1,
                    gdk::ScrollDirection::Smooth => {
                        let (_, dy) = ev.delta();
                        if dy < 0.0 { 1 } else if dy > 0.0 { -1 } else { 0 }
                    }
                    _ => 0,
                };
                if step != 0 {
                    p.emit(PaneEvent::Zoom(step));
                }
                glib::Propagation::Stop
            });
        }
        for w in [self.tree.upcast_ref::<gtk::Widget>(), self.icons.upcast_ref()] {
            let weak = self.weak.borrow().clone();
            w.connect_key_press_event(move |_, ev| {
                let Some(p) = weak.upgrade() else { return glib::Propagation::Proceed };
                p.cancel_pending_selection();
                let key = ev.keyval();
                let mods = ev.state() & gtk::accelerator_get_default_mod_mask();
                if key == gdk::keys::constants::BackSpace && mods.is_empty() {
                    p.go_up();
                    return glib::Propagation::Stop;
                }
                if key == gdk::keys::constants::Menu || (key == gdk::keys::constants::F10 && mods == gdk::ModifierType::SHIFT_MASK) {
                    p.emit(PaneEvent::ContextMenu(None));
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
    }

    fn handle_button(
        &self,
        ev: &gdk::EventButton,
        hit: Option<gtk::TreePath>,
        is_selected: impl Fn(&gtk::TreePath) -> bool,
        select_only: impl Fn(&gtk::TreePath),
    ) -> glib::Propagation {
        if ev.event_type() != gdk::EventType::ButtonPress {
            return glib::Propagation::Proceed;
        }
        self.cancel_pending_selection();
        match ev.button() {
            // A plain click into empty space: nothing selected any more. The press still
            // goes on to the view, so dragging from there draws a selection rectangle.
            1 if hit.is_none() && (ev.state() & gtk::accelerator_get_default_mod_mask()).is_empty() => {
                self.clicked_empty();
                glib::Propagation::Proceed
            }
            3 => {
                match &hit {
                    Some(path) if !is_selected(path) => select_only(path),
                    Some(_) => {}
                    None => self.unselect_all(),
                }
                self.emit(PaneEvent::ContextMenu(Some(ev.clone())));
                glib::Propagation::Stop
            }
            2 => {
                if let Some(path) = hit {
                    if let Some(item) = self.item_at(&path) {
                        if item.is_dir_like() {
                            if let Some(loc) = self.location().child(&item.display_name()) {
                                self.emit(PaneEvent::OpenInNewTab(loc));
                            }
                        }
                    }
                }
                glib::Propagation::Stop
            }
            8 => {
                self.go_back();
                glib::Propagation::Stop
            }
            9 => {
                self.go_forward();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    /// Clearing the selection by clicking where there is no file or folder; the keyboard
    /// focus comes to the list (which also ends typing in the path bar).
    pub fn clicked_empty(&self) {
        self.unselect_all();
        self.focus_view();
        self.emit(PaneEvent::ClickedEmpty);
    }

    fn item_at(&self, path: &gtk::TreePath) -> Option<Item> {
        let iter = self.store.iter(path)?;
        let key: u64 = self.store.value(&iter, model::COL_KEY as i32).get().unwrap_or_default();
        self.st.borrow().rows.get(&key).map(|(i, _)| i.clone())
    }

    fn activate_paths(&self, paths: &[gtk::TreePath]) {
        let items: Vec<Item> = paths.iter().filter_map(|p| self.item_at(p)).collect();
        self.activate(items);
    }

    /// Opens the selection: one folder is entered, everything else goes to the window.
    pub fn activate(&self, items: Vec<Item>) {
        // Search results: folders and archive entries are opened where they are.
        if let [one] = items.as_slice() {
            match one {
                Item::Found(e, _) if e.is_dir_like() => {
                    self.navigate(Location::Dir(e.path.clone()));
                    return;
                }
                Item::FoundInArchive(a, n, _) => {
                    let target = if n.is_dir { a.with_inner(&n.path) } else { a.clone() };
                    let select = if n.is_dir { Vec::new() } else { vec![OsString::from(&n.name)] };
                    self.navigate(Location::Archive(target));
                    if !select.is_empty() {
                        self.select_when_present(select);
                    }
                    return;
                }
                _ => {}
            }
        }
        if items.len() == 1 && items[0].is_dir_like() {
            if let Some(loc) = self.location().child(&items[0].display_name()) {
                self.navigate(loc);
            }
            return;
        }
        if let Some(loc) = items.iter().find_map(|i| backend::enter_as_folder(&self.location(), i)) {
            if items.len() == 1 {
                self.navigate(loc);
                return;
            }
        }
        let files: Vec<Item> = items.into_iter().filter(|i| !i.is_dir_like()).collect();
        if !files.is_empty() {
            self.emit(PaneEvent::OpenFiles(files));
        }
    }

    /// Updates the "Total Size" cell of a folder row.
    pub fn set_dir_size(&self, name: &OsString, bytes: Option<u64>, partial: bool) {
        let st = self.st.borrow();
        let Some((_, iter)) = st.by_name.get(name).and_then(|k| st.rows.get(k)) else { return };
        let (value, text) = match bytes {
            Some(b) => (b as i64, if partial { format!("≥ {}", failbrauwser::fs::format::human_size(b)) } else { failbrauwser::fs::format::human_size(b) }),
            None => (-1, "…".to_string()),
        };
        self.store.set(iter, &[(model::COL_DIRSIZE, &value), (model::COL_DIRSIZE_TEXT, &text)]);
    }

    /// Folders whose total size is not known yet: (name, path on disk).
    pub fn dirs_needing_size(&self) -> Vec<(OsString, std::path::PathBuf)> {
        let st = self.st.borrow();
        st.rows
            .values()
            .filter(|(i, iter)| {
                i.is_dir_like() && self.store.value(iter, model::COL_DIRSIZE as i32).get::<i64>().unwrap_or(-1) < 0
                    && self.store.value(iter, model::COL_DIRSIZE_TEXT as i32).get::<String>().unwrap_or_default().is_empty()
            })
            .filter_map(|(i, _)| i.path().map(|p| (i.os_name(), p.clone())))
            .collect()
    }

    /// The text a column shows for the row `name` (tests).
    pub fn cell_text(&self, name: &str, column: u32) -> Option<String> {
        let st = self.st.borrow();
        let (_, iter) = st.by_name.get(&OsString::from(name)).and_then(|k| st.rows.get(k))?;
        self.store.value(iter, column as i32).get::<String>().ok()
    }

    /// (files, folders) shown right now.
    pub fn counts(&self) -> (usize, usize) {
        let st = self.st.borrow();
        let dirs = st.rows.values().filter(|(i, _)| i.is_dir_like()).count();
        (st.rows.len() - dirs, dirs)
    }

    /// Names of the folders shown.
    pub fn dir_names(&self) -> Vec<OsString> {
        self.st.borrow().rows.values().filter(|(i, _)| i.is_dir_like()).map(|(i, _)| i.os_name()).collect()
    }

    /// Sets the total size of rows by name (archive folders, where sizes are known).
    pub fn set_sizes(&self, sizes: &[(OsString, u64)]) {
        for (name, bytes) in sizes {
            self.set_dir_size(name, Some(*bytes), false);
        }
    }

    pub fn generation(&self) -> u64 {
        self.st.borrow().generation
    }

}
