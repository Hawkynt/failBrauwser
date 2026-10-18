//! The Drives page: every volume with its fill level and mount point. Mounting, unmounting
//! and ejecting are asynchronous UDisks2 calls, so the window never freezes while a slow
//! USB stick writes back its cache.

use super::pane::Pane;
use super::util;
use super::window::Window;
use failbrauwser::devices::{self, DeviceKind};
use failbrauwser::drives::{self, Usage, Volume};
use failbrauwser::fs::format::human_size;
use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

const UDISKS: &str = "org.freedesktop.UDisks2";

pub struct DrivesPage {
    root: gtk::ScrolledWindow,
    list: gtk::ListBox,
    pane: Weak<Pane>,
    window: Weak<Window>,
    /// Change notifications from UDisks2, only while the page is shown (dropped = unsubscribed).
    subscription: RefCell<Option<gio::SignalSubscription>>,
    refresh_pending: std::cell::Cell<bool>,
    /// Volumes shown, in row order, with their fill levels.
    volumes: RefCell<Vec<Volume>>,
    usages: RefCell<Vec<Option<Usage>>>,
    /// Block objects with an operation running (their buttons are disabled).
    busy: RefCell<Vec<String>>,
    weak: RefCell<Weak<DrivesPage>>,
}

thread_local! {
    static PAGES: RefCell<Vec<Rc<DrivesPage>>> = const { RefCell::new(Vec::new()) };
}

fn page_for(pane: &Rc<Pane>) -> Option<Rc<DrivesPage>> {
    PAGES.with(|p| {
        let mut p = p.borrow_mut();
        p.retain(|x| x.pane.strong_count() > 0);
        p.iter().find(|x| x.pane.upgrade().is_some_and(|q| Rc::ptr_eq(&q, pane))).cloned()
    })
}

pub fn attach(w: &Window, pane: &Rc<Pane>) {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_activate_on_single_click(false);
    list.set_header_func(Some(Box::new(|row, before| {
        let group = |r: &gtk::ListBoxRow| r.widget_name().to_string();
        let g = group(row);
        if before.is_none_or(|b| group(b) != g) {
            let label = gtk::Label::new(None);
            label.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(if g == "removable" { "Removable Drives" } else { "This Computer" })));
            label.set_xalign(0.0);
            label.set_margin_start(12);
            label.set_margin_top(12);
            label.set_margin_bottom(4);
            label.style_context().add_class("dim-label");
            label.show();
            row.set_header(Some(&label));
        } else {
            row.set_header(None::<&gtk::Widget>);
        }
    })));
    let root = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    root.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    root.add(&list);
    root.show_all();
    pane.stack.add_named(&root, "drives");
    let page = Rc::new(DrivesPage {
        root,
        list,
        pane: Rc::downgrade(pane),
        window: Rc::downgrade(&w.me()),
        subscription: RefCell::new(None),
        refresh_pending: std::cell::Cell::new(false),
        volumes: RefCell::new(Vec::new()),
        usages: RefCell::new(Vec::new()),
        busy: RefCell::new(Vec::new()),
        weak: RefCell::new(Weak::new()),
    });
    *page.weak.borrow_mut() = Rc::downgrade(&page);
    super::phones::watch();
    let weak = Rc::downgrade(&page);
    super::phones::connect_changed(move || {
        let Some(p) = weak.upgrade() else { return false };
        // Only while the page is shown, like the UDisks2 notifications.
        if p.subscription.borrow().is_some() {
            p.schedule_refresh();
        }
        true
    });
    let weak = Rc::downgrade(&page);
    page.list.connect_row_activated(move |_, row| {
        if let Some(p) = weak.upgrade() {
            if let Some(v) = p.volumes.borrow().get(row.index() as usize).cloned() {
                p.open(&v);
            }
        }
    });
    let weak = Rc::downgrade(&page);
    page.list.connect_button_press_event(move |list, ev| {
        let Some(p) = weak.upgrade() else { return glib::Propagation::Proceed };
        // A plain click below the drives clears the selection, like in a folder.
        if ev.button() == 1 && ev.event_type() == gdk::EventType::ButtonPress && p.press_left(ev.position().1 as i32) {
            return glib::Propagation::Stop;
        }
        if ev.button() != 3 {
            return glib::Propagation::Proceed;
        }
        if let Some(row) = list.row_at_y(ev.position().1 as i32) {
            list.select_row(Some(&row));
            p.context_menu(row.index() as usize, ev);
        }
        glib::Propagation::Stop
    });
    PAGES.with(|p| p.borrow_mut().push(page));
}

/// Properties of the drive selected on this pane's page (Alt+Enter).
pub fn show_selected_properties(pane: &Rc<Pane>) {
    let Some(page) = page_for(pane) else { return };
    let idx = page.list.selected_row().map(|r| r.index() as usize).unwrap_or(0);
    let v = page.volumes.borrow().get(idx).cloned();
    let u = page.usages.borrow().get(idx).copied().flatten();
    if let Some(v) = v {
        page.show_properties(&v, u);
    }
}

/// Opens the properties of the volume with this title (tests).
pub fn show_properties_of(pane: &Rc<Pane>, title: &str) -> bool {
    let Some(page) = page_for(pane) else { return false };
    let found = page.volumes.borrow().iter().position(|v| v.title() == title);
    match found {
        Some(i) => {
            let v = page.volumes.borrow()[i].clone();
            let u = page.usages.borrow().get(i).copied().flatten();
            page.show_properties(&v, u);
            true
        }
        None => false,
    }
}

/// Shows or hides the page as the pane's location changes.
pub fn location_changed(pane: &Rc<Pane>) {
    let Some(page) = page_for(pane) else { return };
    if pane.location() == Location::Drives {
        pane.stack.set_visible_child(&page.root);
        page.subscribe();
        page.refresh();
        page.list.grab_focus();
    } else {
        page.unsubscribe();
        if pane.stack.visible_child().as_ref() == Some(page.root.upcast_ref()) {
            pane.apply_view_mode();
        }
    }
}

/// The volumes on the page of this pane (tests).
pub fn shown_volumes(pane: &Rc<Pane>) -> Vec<Volume> {
    page_for(pane).map(|p| p.volumes.borrow().clone()).unwrap_or_default()
}

/// Clicks below the last drive of this pane's page (tests); false without a page.
pub fn click_empty(pane: &Rc<Pane>) -> bool {
    let Some(page) = page_for(pane) else { return false };
    let bottom = page.list.children().last().map(|r| r.allocation().y() + r.allocation().height()).unwrap_or(0);
    page.press_left(bottom + 5)
}

/// The texts of the rows of drives with this title, one string per row (tests).
pub fn row_texts(pane: &Rc<Pane>, title: &str) -> Vec<String> {
    let Some(page) = page_for(pane) else { return Vec::new() };
    let vols = page.volumes.borrow();
    page.list
        .children()
        .iter()
        .enumerate()
        .filter(|(i, _)| vols.get(*i).is_some_and(|v| v.title() == title))
        .map(|(_, row)| labels_in(row).join(" | "))
        .collect()
}

fn labels_in(w: &gtk::Widget) -> Vec<String> {
    if let Some(l) = w.downcast_ref::<gtk::Label>() {
        return vec![l.text().to_string()];
    }
    if let Some(b) = w.downcast_ref::<gtk::Button>() {
        return b.label().map(|l| vec![l.replace('_', "")]).unwrap_or_default();
    }
    w.downcast_ref::<gtk::Container>().map(|c| c.children().iter().flat_map(labels_in).collect()).unwrap_or_default()
}

/// Clicks a button (by its text) in the first row of the drive with this title whose texts
/// contain `row_text` (tests).
pub fn click_button(pane: &Rc<Pane>, title: &str, row_text: &str, button: &str) -> Result<(), String> {
    let page = page_for(pane).ok_or("no drives page")?;
    let rows = page.list.children();
    let i = {
        let vols = page.volumes.borrow();
        (0..rows.len())
            .find(|&i| vols.get(i).is_some_and(|v| v.title() == title) && labels_in(&rows[i]).join(" | ").contains(row_text))
            .ok_or_else(|| format!("no drive {title:?} showing {row_text:?}"))?
    };
    fn find(w: &gtk::Widget, label: &str) -> Option<gtk::Button> {
        if let Some(b) = w.downcast_ref::<gtk::Button>() {
            if b.label().is_some_and(|l| l.replace('_', "") == label) {
                return Some(b.clone());
            }
        }
        w.downcast_ref::<gtk::Container>()?.children().iter().find_map(|c| find(c, label))
    }
    let b = find(&rows[i], button).ok_or_else(|| format!("no button {button:?}"))?;
    if !b.is_sensitive() {
        return Err(format!("button {button:?} is disabled"));
    }
    b.clicked();
    Ok(())
}

/// Selects the drive with this title (tests).
pub fn select(pane: &Rc<Pane>, title: &str) -> bool {
    let Some(page) = page_for(pane) else { return false };
    let Some(i) = page.volumes.borrow().iter().position(|v| v.title() == title) else { return false };
    page.list.select_row(page.list.row_at_index(i as i32).as_ref());
    true
}

/// The title of the selected drive, if one is (tests).
pub fn selected(pane: &Rc<Pane>) -> Option<String> {
    let page = page_for(pane)?;
    let i = page.list.selected_row()?.index() as usize;
    page.volumes.borrow().get(i).map(Volume::title)
}

impl DrivesPage {
    /// A left click at `y` in the list; true when it hit no drive and was handled.
    fn press_left(&self, y: i32) -> bool {
        if self.list.row_at_y(y).is_some() {
            return false;
        }
        self.clicked_empty();
        true
    }

    fn clicked_empty(&self) {
        self.list.unselect_all();
        self.list.grab_focus();
        if let Some(pane) = self.pane.upgrade() {
            pane.emit(super::pane::PaneEvent::ClickedEmpty);
        }
    }

    fn me(&self) -> Rc<DrivesPage> {
        self.weak.borrow().upgrade().expect("page alive")
    }

    fn subscribe(&self) {
        if self.subscription.borrow().is_some() {
            return;
        }
        let Ok(conn) = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE) else { return };
        let weak = self.weak.borrow().clone();
        let sub = conn.subscribe_to_signal(Some(UDISKS), None, None, None, None, gio::DBusSignalFlags::NONE, move |_| {
            if let Some(p) = weak.upgrade() {
                p.schedule_refresh();
            }
        });
        *self.subscription.borrow_mut() = Some(sub);
    }

    fn unsubscribe(&self) {
        self.subscription.borrow_mut().take();
    }

    fn schedule_refresh(&self) {
        if self.refresh_pending.replace(true) {
            return;
        }
        let weak = self.weak.borrow().clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
            if let Some(p) = weak.upgrade() {
                p.refresh_pending.set(false);
                p.refresh();
            }
        });
    }

    /// Reads the volume list from UDisks2 (or the mount table) and redraws.
    pub fn refresh(&self) {
        let me = self.me();
        glib::spawn_future_local(async move {
            let vols = match query_udisks().await {
                Some(v) => v,
                None => gio::spawn_blocking(drives::read_mountinfo).await.unwrap_or_default(),
            };
            // statvfs can hang on a dead network mount: never on the UI thread.
            let mut with_usage = gio::spawn_blocking(move || {
                vols.into_iter().map(|v| {
                    let u = v.mount_point().and_then(|m| drives::usage(m));
                    (v, u)
                }).collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            // Phones next to the USB sticks (their fill levels asked of GVfs, not statvfs).
            with_usage.extend(super::phones::volumes().await);
            me.rebuild(with_usage);
        });
    }

    fn rebuild(&self, vols: Vec<(Volume, Option<Usage>)>) {
        let selected = self.list.selected_row().map(|r| r.index());
        for child in self.list.children() {
            self.list.remove(&child);
        }
        *self.volumes.borrow_mut() = vols.iter().map(|(v, _)| v.clone()).collect();
        *self.usages.borrow_mut() = vols.iter().map(|(_, u)| *u).collect();
        for (v, u) in &vols {
            let row = self.make_row(v, u.as_ref());
            self.list.add(&row);
        }
        self.list.show_all();
        if let Some(r) = selected.and_then(|i| self.list.row_at_index(i)) {
            self.list.select_row(Some(&r));
        }
        if let (Some(p), Some(w)) = (self.pane.upgrade(), self.window.upgrade()) {
            if Rc::ptr_eq(&w.current_pane(), &p) {
                w.update_status();
            }
        }
    }

    fn make_row(&self, v: &Volume, usage: Option<&Usage>) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(if v.removable || v.usb { "removable" } else { "fixed" });
        let grid = gtk::Grid::new();
        grid.set_column_spacing(12);
        grid.set_row_spacing(3);
        grid.set_margin_start(12);
        grid.set_margin_end(12);
        grid.set_margin_top(8);
        grid.set_margin_bottom(8);

        let icon = gtk::Image::from_gicon(&gio::ThemedIcon::from_names(&v.icon_names()), gtk::IconSize::Dialog);
        icon.set_valign(gtk::Align::Center);
        grid.attach(&icon, 0, 0, 1, 4);

        let title = gtk::Label::new(None);
        title.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&v.title())));
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        grid.attach(&title, 1, 0, 1, 1);

        // A phone: which storage and what kind of device, not its address.
        let mut sub = if v.is_device() { vec![v.label.clone(), drives::drive_kind(v)] } else { vec![v.device.clone()] };
        sub.retain(|s| !s.is_empty());
        if !v.fs_type.is_empty() {
            sub.push(v.fs_type.clone());
        }
        if !v.drive_name.is_empty() && !v.is_device() {
            sub.push(v.drive_name.clone());
        }
        let subtitle = gtk::Label::new(Some(&sub.join(" · ")));
        subtitle.set_xalign(0.0);
        subtitle.style_context().add_class("dim-label");
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        grid.attach(&subtitle, 1, 1, 1, 1);

        let bar = gtk::LevelBar::for_interval(0.0, 1.0);
        // Theme colors: normal up to 90 % full, the warning color beyond.
        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_LOW));
        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_HIGH));
        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_FULL));
        bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_HIGH, 0.9);
        bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_LOW, 1.0);
        bar.set_size_request(-1, 8);
        match usage {
            Some(u) => bar.set_value(u.fraction()),
            None => bar.set_sensitive(false),
        }
        grid.attach(&bar, 1, 2, 1, 1);

        let text = match (usage, v.mount_point()) {
            (Some(u), Some(_)) if v.is_device() => format!("{} free of {} — {:.0} % used", human_size(u.free), human_size(u.total), u.fraction() * 100.0),
            _ if no_storage(v) => devices::no_storage_hint(DeviceKind::Phone).to_string(),
            (None, Some(_)) if v.is_device() => "Connected".to_string(),
            (None, None) if v.is_device() => "Connected — open it to see its files".to_string(),
            (Some(u), Some(m)) => format!("{} free of {} — {:.0} % used — {}", human_size(u.free), human_size(u.total), u.fraction() * 100.0, m.display()),
            (None, Some(m)) => format!("Mounted at {}", m.display()),
            _ if v.fs_type.starts_with("crypto") => format!("Encrypted, locked — {}", human_size(v.size)),
            _ => format!("Not mounted — {}", human_size(v.size)),
        };
        let info = gtk::Label::new(Some(&text));
        info.set_xalign(0.0);
        info.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        grid.attach(&info, 1, 3, 1, 1);

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_valign(gtk::Align::Center);
        let busy = self.busy.borrow().contains(&busy_key(v));
        let add = |label: &str, tip: &str, f: Box<dyn Fn(&DrivesPage)>| {
            let b = gtk::Button::with_mnemonic(label);
            b.set_tooltip_text(Some(tip));
            b.set_sensitive(!busy);
            let weak = self.weak.borrow().clone();
            b.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    f(&p);
                }
            });
            buttons.pack_start(&b, false, false, 0);
        };
        let vol = v.clone();
        if no_storage(v) {
            add("_Retry", "Connect again, after unlocking the phone", Box::new(move |p| p.reconnect(&vol)));
        } else if v.mount_point().is_some() {
            add("_Open", "Show the files on this drive", Box::new(move |p| p.open(&vol)));
        } else if v.is_device() {
            add("_Open", "Connect and show the files on this device", Box::new(move |p| p.open(&vol)));
        } else if v.mountable && !v.block_object.is_empty() {
            add("_Mount", "Mount and open this drive", Box::new(move |p| p.open(&vol)));
        }
        let is_root = v.mount_points.iter().any(|m| m == Path::new("/"));
        if v.mount_point().is_some() && !is_root && !v.block_object.is_empty() && !v.system {
            let vol = v.clone();
            add("_Unmount", "Unmount this drive", Box::new(move |p| p.unmount(&vol, false)));
        }
        if (v.usb || v.ejectable) && !v.drive_object.is_empty() {
            let vol = v.clone();
            add("_Eject", "Unmount and power off, safe to unplug", Box::new(move |p| p.unmount(&vol, true)));
        } else if v.is_device() && v.mount_point().is_some() {
            let vol = v.clone();
            add("_Eject", "Close the connection, safe to unplug", Box::new(move |p| p.unmount(&vol, true)));
        }
        let info = gtk::Button::from_icon_name(Some("document-properties-symbolic"), gtk::IconSize::Button);
        info.set_tooltip_text(Some("Properties"));
        let (vol, u) = (v.clone(), usage.copied());
        let weak = self.weak.borrow().clone();
        info.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.show_properties(&vol, u);
            }
        });
        buttons.pack_start(&info, false, false, 0);
        if busy {
            let spinner = gtk::Spinner::new();
            spinner.start();
            buttons.pack_start(&spinner, false, false, 0);
        }
        grid.attach(&buttons, 2, 0, 1, 4);
        row.add(&grid);
        row
    }

    fn context_menu(&self, index: usize, ev: &gtk::gdk::EventButton) {
        let Some(v) = self.volumes.borrow().get(index).cloned() else { return };
        let u = self.usages.borrow().get(index).copied().flatten();
        let menu = gtk::Menu::new();
        let add = |label: &str, f: Box<dyn Fn(&DrivesPage)>| {
            let item = gtk::MenuItem::with_mnemonic(label);
            let weak = self.weak.borrow().clone();
            item.connect_activate(move |_| {
                if let Some(p) = weak.upgrade() {
                    f(&p);
                }
            });
            menu.append(&item);
        };
        let vol = v.clone();
        if no_storage(&v) {
            add("_Retry", Box::new(move |p| p.reconnect(&vol)));
        } else if v.mount_point().is_some() || v.is_device() {
            add("_Open", Box::new(move |p| p.open(&vol)));
        } else if v.mountable {
            add("_Mount", Box::new(move |p| p.open(&vol)));
        }
        let is_root = v.mount_points.iter().any(|m| m == Path::new("/"));
        if v.mount_point().is_some() && !is_root && !v.system && !v.block_object.is_empty() {
            let vol = v.clone();
            add("_Unmount", Box::new(move |p| p.unmount(&vol, false)));
        }
        if (v.usb || v.ejectable) && !v.drive_object.is_empty() || v.is_device() && v.mount_point().is_some() {
            let vol = v.clone();
            add("_Eject", Box::new(move |p| p.unmount(&vol, true)));
        }
        menu.append(&gtk::SeparatorMenuItem::new());
        let vol = v.clone();
        add("_Properties…", Box::new(move |p| p.show_properties(&vol, u)));
        menu.set_attach_widget(Some(&self.list));
        menu.show_all();
        menu.popup_at_pointer(Some(ev));
    }

    /// Everything known about a volume, its drive and how full it is.
    fn show_properties(&self, v: &Volume, usage: Option<Usage>) {
        let Some(w) = self.window.upgrade() else { return };
        let d = gtk::Dialog::with_buttons(Some(&format!("{} — Properties", v.title())), Some(&w.win), gtk::DialogFlags::DESTROY_WITH_PARENT, &[]);
        d.set_resizable(false);
        let area = d.content_area();
        area.set_border_width(12);
        area.set_spacing(12);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        header.pack_start(&gtk::Image::from_gicon(&gio::ThemedIcon::from_names(&v.icon_names()), gtk::IconSize::Dialog), false, false, 0);
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(None);
        title.set_markup(&format!("<big><b>{}</b></big>", glib::markup_escape_text(&v.title())));
        title.set_xalign(0.0);
        let mut sub: Vec<String> = Vec::new();
        let kind = drives::drive_kind(v);
        if !kind.is_empty() {
            sub.push(kind);
        }
        if v.size > 0 {
            sub.push(human_size(v.size));
        }
        if !v.fs_type.is_empty() {
            sub.push(v.fs_type.clone());
        }
        let subtitle = gtk::Label::new(Some(&sub.join(" · ")));
        subtitle.set_xalign(0.0);
        subtitle.style_context().add_class("dim-label");
        titles.pack_start(&title, false, false, 0);
        titles.pack_start(&subtitle, false, false, 0);
        header.pack_start(&titles, true, true, 0);
        area.add(&header);

        // How full: a bar with used / free / total underneath.
        if let Some(u) = usage {
            let bar = gtk::LevelBar::for_interval(0.0, 1.0);
            bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_LOW));
            bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_HIGH));
            bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_FULL));
            bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_HIGH, 0.9);
            bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_LOW, 1.0);
            bar.set_value(u.fraction());
            bar.set_size_request(360, 12);
            area.add(&bar);
            let counts = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            counts.set_homogeneous(true);
            for (label, value) in [
                ("Used", format!("{} ({:.0} %)", human_size(u.used), u.fraction() * 100.0)),
                ("Free", human_size(u.free)),
                ("Total", human_size(u.total)),
            ] {
                let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
                let k = gtk::Label::new(Some(label));
                k.style_context().add_class("dim-label");
                let val = gtk::Label::new(None);
                val.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&value)));
                b.pack_start(&k, false, false, 0);
                b.pack_start(&val, false, false, 0);
                counts.pack_start(&b, true, true, 0);
            }
            area.add(&counts);
        }

        let grid = gtk::Grid::new();
        grid.set_column_spacing(12);
        grid.set_row_spacing(4);
        let mut y = 0;
        let mut section = |name: &str, rows: Vec<(&str, String)>| {
            let rows: Vec<_> = rows.into_iter().filter(|(_, v)| !v.is_empty()).collect();
            if rows.is_empty() {
                return;
            }
            let h = gtk::Label::new(None);
            h.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(name)));
            h.set_xalign(0.0);
            h.set_margin_top(if y == 0 { 0 } else { 8 });
            grid.attach(&h, 0, y, 2, 1);
            y += 1;
            for (k, val) in rows {
                let kl = gtk::Label::new(Some(k));
                kl.set_xalign(1.0);
                kl.set_yalign(0.0);
                kl.style_context().add_class("dim-label");
                let vl = gtk::Label::new(Some(&val));
                vl.set_xalign(0.0);
                vl.set_selectable(true);
                vl.set_line_wrap(true);
                vl.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
                vl.set_max_width_chars(44);
                grid.attach(&kl, 0, y, 1, 1);
                grid.attach(&vl, 1, y, 1, 1);
                y += 1;
            }
        };
        let fs = match (v.fs_type.as_str(), v.fs_version.as_str()) {
            (t, "") => t.to_string(),
            (t, ver) => format!("{t} {ver}"),
        };
        let mounted = v.mount_points.iter().map(|m| m.display().to_string()).collect::<Vec<_>>().join("\n");
        let options = v.mount_point().and_then(|m| drives::mount_options(m)).unwrap_or_default();
        let yes_no = |b: bool| if b { "Yes".to_string() } else { "No".to_string() };
        section(
            "Volume",
            vec![
                ("Label:", v.label.clone()),
                ("Device:", v.device.clone()),
                ("File system:", fs),
                ("UUID:", v.uuid.clone()),
                ("Mounted at:", if mounted.is_empty() { "Not mounted".into() } else { mounted }),
                ("Mount options:", options),
                ("Read-only:", yes_no(v.read_only || v.mount_point().and_then(|m| drives::mount_options(m)).is_some_and(|o| o.split(',').any(|x| x == "ro")))),
            ],
        );
        section(
            "Partition",
            vec![
                ("Number:", if v.partition_number > 0 { v.partition_number.to_string() } else { String::new() }),
                ("Name:", v.partition_name.clone()),
                ("Type:", if v.partition_type.is_empty() { String::new() } else { drives::partition_type_name(&v.partition_type) }),
            ],
        );
        section(
            "Drive",
            vec![
                ("Model:", v.drive_name.clone()),
                ("Serial number:", v.serial.clone()),
                ("Firmware:", v.revision.clone()),
                ("Kind:", drives::drive_kind(v)),
                ("Connection:", match v.bus.as_str() {
                    "usb" if v.is_device() => format!("USB ({})", v.protocol.to_uppercase()),
                    "usb" => "USB".into(),
                    "sdio" => "SD card reader".into(),
                    "ieee1394" => "FireWire".into(),
                    "" if v.device.starts_with("/dev/nvme") => "NVMe".into(),
                    "" if !v.drive_object.is_empty() => "Internal".into(),
                    other => other.to_string(),
                }),
                ("Drive size:", if v.drive_size > 0 { human_size(v.drive_size) } else { String::new() }),
                ("Removable:", if v.drive_object.is_empty() { String::new() } else { yes_no(v.removable || v.usb) }),
            ],
        );
        area.add(&grid);

        if v.mount_point().is_some() && !no_storage(v) || v.is_device() {
            d.add_button("_Open", gtk::ResponseType::Accept);
        } else if v.mountable && !v.block_object.is_empty() {
            d.add_button("_Mount and Open", gtk::ResponseType::Accept);
        }
        d.add_button("_Close", gtk::ResponseType::Close);
        d.set_default_response(gtk::ResponseType::Close);
        let weak = self.weak.borrow().clone();
        let vol = v.clone();
        d.connect_response(move |d, resp| {
            if resp == gtk::ResponseType::Accept {
                if let Some(p) = weak.upgrade() {
                    p.open(&vol);
                }
            }
            d.close();
        });
        d.show_all();
    }

    fn open(&self, v: &Volume) {
        if v.is_device() && (v.mount_point().is_none() || no_storage(v)) {
            self.open_device(v);
            return;
        }
        if let Some(m) = v.mount_point() {
            self.navigate(Location::Dir(m.clone()));
            return;
        }
        if !v.mountable || v.block_object.is_empty() {
            return;
        }
        let me = self.me();
        let obj = v.block_object.clone();
        self.set_busy(&obj, true);
        glib::spawn_future_local(async move {
            let res = call(&obj, "org.freedesktop.UDisks2.Filesystem", "Mount", empty_options()).await;
            me.set_busy(&obj, false);
            match res {
                Ok(reply) => {
                    let path: String = reply.child_value(0).get().unwrap_or_default();
                    me.navigate(Location::Dir(PathBuf::from(path)));
                }
                Err(e) => me.error("Cannot mount drive", &e),
            }
        });
    }

    /// Connects to a phone or camera and shows its files.
    fn open_device(&self, v: &Volume) {
        let Some(w) = self.window.upgrade() else { return };
        let me = self.me();
        let key = busy_key(v);
        self.set_busy(&key, true);
        super::phones::open(w.win.upcast_ref(), &v.device, move |res| {
            me.set_busy(&key, false);
            match res {
                Ok(path) => me.navigate(Location::Dir(path)),
                Err(msg) if msg.is_empty() => {}
                Err(msg) => me.error("Cannot open the device", &msg),
            }
        });
    }

    /// Closes the connection to a device that showed no storage and opens it again (once
    /// the phone is unlocked, its storages show up).
    fn reconnect(&self, v: &Volume) {
        let Some(w) = self.window.upgrade() else { return };
        let me = self.me();
        let vol = v.clone();
        self.set_busy(&busy_key(v), true);
        super::phones::eject(w.win.upcast_ref(), &v.device, move |_| {
            me.set_busy(&busy_key(&vol), false);
            me.open_device(&vol);
        });
    }

    /// Unmounts (and for `eject`, powers off / ejects the whole drive). Tabs showing the
    /// drive are moved to the drives page first, so they do not keep it busy.
    fn unmount(&self, v: &Volume, eject: bool) {
        if v.is_device() {
            self.eject_device(v);
            return;
        }
        let mounts: Vec<PathBuf> = if eject {
            self.volumes.borrow().iter().filter(|x| x.drive_object == v.drive_object).flat_map(|x| x.mount_points.clone()).collect()
        } else {
            v.mount_points.clone()
        };
        if let Some(w) = self.window.upgrade() {
            leave(&w, &mounts);
        }
        let targets: Vec<String> = if eject {
            self.volumes.borrow().iter().filter(|x| x.drive_object == v.drive_object && !x.mount_points.is_empty()).map(|x| x.block_object.clone()).collect()
        } else {
            vec![v.block_object.clone()]
        };
        let me = self.me();
        let v = v.clone();
        self.set_busy(&v.block_object, true);
        glib::spawn_future_local(async move {
            for obj in &targets {
                if let Err(e) = call(obj, "org.freedesktop.UDisks2.Filesystem", "Unmount", empty_options()).await {
                    me.set_busy(&v.block_object, false);
                    me.error("Cannot unmount drive", &e);
                    return;
                }
            }
            if eject {
                let method = if v.usb { "PowerOff" } else { "Eject" };
                if let Err(e) = call(&v.drive_object, "org.freedesktop.UDisks2.Drive", method, empty_options()).await {
                    // Unmounted but not powered off: still safe, but say so.
                    me.error("Drive unmounted, but could not be powered off", &e);
                }
            }
            me.set_busy(&v.block_object, false);
            me.refresh();
        });
    }

    /// Closes the connection to a phone or camera; tabs showing its files go to the drives
    /// page first.
    fn eject_device(&self, v: &Volume) {
        let Some(w) = self.window.upgrade() else { return };
        if let Some(root) = super::phones::root_path(&v.device) {
            leave(&w, &[root]);
        }
        let me = self.me();
        let key = busy_key(v);
        self.set_busy(&key, true);
        super::phones::eject(w.win.upcast_ref(), &v.device, move |res| {
            me.set_busy(&key, false);
            match res {
                Ok(()) => me.refresh(),
                Err(msg) if msg.is_empty() => {}
                Err(msg) => me.error("Cannot eject the device", &msg),
            }
        });
    }

    fn set_busy(&self, obj: &str, on: bool) {
        {
            let mut b = self.busy.borrow_mut();
            b.retain(|x| x != obj);
            if on {
                b.push(obj.to_string());
            }
        }
        self.refresh();
    }

    fn navigate(&self, loc: Location) {
        if let Some(p) = self.pane.upgrade() {
            p.navigate(loc);
        }
    }

    fn error(&self, title: &str, msg: &str) {
        if let Some(w) = self.window.upgrade() {
            util::show_error(&w.win, title, msg);
        }
    }
}

/// What marks a volume as busy: its UDisks2 object, or a device's address.
fn busy_key(v: &Volume) -> String {
    if v.is_device() { v.device.clone() } else { v.block_object.clone() }
}

/// A phone that is open but shows no storage (locked, or access not allowed yet).
fn no_storage(v: &Volume) -> bool {
    v.device_kind() == Some(DeviceKind::Phone) && v.mount_point().is_some() && v.label.is_empty()
}

/// Moves every tab showing a folder below one of `mounts` to the drives page.
fn leave(w: &Window, mounts: &[PathBuf]) {
    for win in w.app.windows() {
        for p in win.panes() {
            if p.location().nearest_dir().is_some_and(|d| mounts.iter().any(|m| d.starts_with(m))) {
                p.navigate(Location::Drives);
            }
        }
    }
}

fn empty_options() -> glib::Variant {
    glib::Variant::tuple_from_iter([glib::VariantDict::new(None).end()])
}

async fn call(object: &str, iface: &str, method: &str, args: glib::Variant) -> Result<glib::Variant, String> {
    let conn = gio::bus_get_future(gio::BusType::System).await.map_err(|e| e.to_string())?;
    conn.call_future(Some(UDISKS), object, iface, method, Some(&args), None, gio::DBusCallFlags::ALLOW_INTERACTIVE_AUTHORIZATION, -1)
        .await
        .map_err(|e| gio::DBusError::remote_error(&e).map(|_| strip_dbus_prefix(&e.to_string())).unwrap_or_else(|| e.to_string()))
}

fn strip_dbus_prefix(msg: &str) -> String {
    // "GDBus.Error:org.freedesktop.UDisks2.Error.DeviceBusy: Error unmounting …: target is busy"
    msg.split_once(": ").map(|(_, rest)| rest.to_string()).unwrap_or_else(|| msg.to_string())
}

async fn query_udisks() -> Option<Vec<Volume>> {
    let conn = gio::bus_get_future(gio::BusType::System).await.ok()?;
    let reply = conn
        .call_future(Some(UDISKS), "/org/freedesktop/UDisks2", "org.freedesktop.DBus.ObjectManager", "GetManagedObjects", None, Some(glib::VariantTy::new("(a{oa{sa{sv}}})").unwrap()), gio::DBusCallFlags::NONE, 5000)
        .await
        .ok()?;
    Some(drives::parse_udisks(&reply))
}
