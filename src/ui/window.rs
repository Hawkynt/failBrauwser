//! A browser window: menu bar, tool bar with location entry, side panel, tabs, status bar.

use super::actions::{self, radio_action, toggle_action};
use super::app::AppCtx;
use super::model::Item;
use super::pane::{Pane, PaneEvent};
use super::util;
use failbrauwser::config::{COLUMN_IDS, TreeRoot, ViewMode};
use failbrauwser::fs::format::human_size;
use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

pub struct Window {
    pub win: gtk::ApplicationWindow,
    pub app: Rc<AppCtx>,
    pub notebook: gtk::Notebook,
    pub location_entry: gtk::Entry,
    pub pathbar: Rc<super::pathbar::PathBar>,
    pub paned: gtk::Paned,
    /// The side panel's container; filled by the sidebar.
    pub side: gtk::Box,
    statusbar: gtk::Statusbar,
    back_btn: gtk::ToolButton,
    fwd_btn: gtk::ToolButton,
    up_btn: gtk::ToolButton,
    panes: RefCell<Vec<Rc<Pane>>>,
    weak: RefCell<Weak<Window>>,
    /// Called whenever the current tab or its location changes (sidebar, tree).
    location_listeners: RefCell<Vec<Box<dyn Fn(&Window)>>>,
}

/// A themed icon with fallbacks, for icon themes that lack the first name. Symbolic
/// icons come first where the theme's full-color ones may vanish on a dark toolbar:
/// GTK paints symbolic icons in the theme's text color.
fn themed(names: &[&str]) -> gtk::Image {
    gtk::Image::from_gicon(&gio::ThemedIcon::from_names(names), gtk::IconSize::SmallToolbar)
}

/// A tool button with a small icon and its label next to it.
fn tool_button(icon: &str, label: &str, tooltip: &str, action: &str) -> gtk::ToolButton {
    let b = gtk::ToolButton::new(None::<&gtk::Widget>, Some(label));
    // In the "both horizontal" style only important items show their label.
    b.set_is_important(true);
    // Symbolic first: drawn in the label's color, so icon and text read alike.
    let symbolic = format!("{icon}-symbolic");
    let image = themed(&[symbolic.as_str(), icon]);
    image.show();
    b.set_icon_widget(Some(&image));
    gtk::prelude::WidgetExt::set_tooltip_text(&b, Some(tooltip));
    b.set_action_name(Some(action));
    b
}

impl Window {
    pub fn new(app: &Rc<AppCtx>, locations: Vec<Location>) -> Rc<Window> {
        let (w, h) = {
            let s = app.settings.borrow();
            (s.window_width, s.window_height)
        };
        let win = gtk::ApplicationWindow::new(&app.app);
        win.set_default_size(w, h);
        win.set_show_menubar(false);

        let menubar = gtk::MenuBar::from_model(&actions::menubar_model());

        let toolbar = gtk::Toolbar::new();
        toolbar.style_context().add_class("primary-toolbar");
        toolbar.set_style(gtk::ToolbarStyle::BothHoriz);
        toolbar.set_icon_size(gtk::IconSize::SmallToolbar);
        let back_btn = tool_button("go-previous", "Back", "Back", "win.back");
        let fwd_btn = tool_button("go-next", "Forward", "Forward", "win.forward");
        let up_btn = tool_button("go-up", "Up", "Open the parent folder", "win.up");
        // Home and Drives live in the side panel (and on Alt+Home / Alt+D).
        for b in [&back_btn, &fwd_btn, &up_btn] {
            toolbar.insert(b, -1);
        }
        let pathbar = super::pathbar::PathBar::new();
        let location_entry = pathbar.entry.clone();
        super::remote::attach(&location_entry);
        let entry_item = gtk::ToolItem::new();
        gtk::prelude::ToolItemExt::set_expand(&entry_item, true);
        entry_item.set_margin_start(4);
        entry_item.set_margin_end(4);
        entry_item.add(&pathbar.root);
        toolbar.insert(&entry_item, -1);
        let reload_btn = tool_button("view-refresh", "Reload", "Reload", "win.reload");
        toolbar.insert(&reload_btn, -1);
        let list_btn = gtk::ToggleToolButton::new();
        list_btn.set_label(Some("List"));
        list_btn.set_is_important(true);
        list_btn.set_icon_widget(Some(&themed(&["view-list-symbolic", "view-list", "view-list-details"])));
        gtk::prelude::WidgetExt::set_tooltip_text(&list_btn, Some("View as detailed list"));
        list_btn.set_action_name(Some("win.view-mode"));
        list_btn.set_action_target_value(Some(&"list".to_variant()));
        let icons_btn = gtk::ToggleToolButton::new();
        icons_btn.set_label(Some("Icons"));
        icons_btn.set_is_important(true);
        icons_btn.set_icon_widget(Some(&themed(&["view-grid-symbolic", "view-app-grid-symbolic", "view-grid", "view-list-icons"])));
        gtk::prelude::WidgetExt::set_tooltip_text(&icons_btn, Some("View as icons"));
        icons_btn.set_action_name(Some("win.view-mode"));
        icons_btn.set_action_target_value(Some(&"icons".to_variant()));
        toolbar.insert(&list_btn, -1);
        toolbar.insert(&icons_btn, -1);

        let notebook = gtk::Notebook::new();
        notebook.set_scrollable(true);
        notebook.set_show_border(false);
        notebook.set_show_tabs(false);

        let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.pack1(&side, false, false);
        paned.pack2(&notebook, true, false);
        paned.set_position(app.settings.borrow().sidebar_width);

        let statusbar = gtk::Statusbar::new();
        statusbar.set_margin_top(0);
        statusbar.set_margin_bottom(0);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vbox.pack_start(&menubar, false, false, 0);
        vbox.pack_start(&toolbar, false, false, 0);
        vbox.pack_start(&paned, true, true, 0);
        vbox.pack_start(&statusbar, false, false, 0);
        win.add(&vbox);

        let window = Rc::new(Window {
            win,
            app: app.clone(),
            notebook,
            location_entry,
            pathbar,
            paned,
            side,
            statusbar,
            back_btn,
            fwd_btn,
            up_btn,
            panes: RefCell::new(Vec::new()),
            weak: RefCell::new(Weak::new()),
            location_listeners: RefCell::new(Vec::new()),
        });
        *window.weak.borrow_mut() = Rc::downgrade(&window);
        window.install_actions();
        window.connect_signals();
        for loc in locations {
            window.add_tab(loc, true);
        }
        window.win.show_all();
        window.side.set_visible(window.app.settings.borrow().sidebar_width > 0);
        window.after_build();
        window.current_pane().focus_view();
        window
    }

    /// Hook for features that attach to a freshly built window (side panel, drives page).
    fn after_build(self: &Rc<Self>) {
        super::summary::attach(self, &self.statusbar);
        super::extensions::window_created(self);
    }

    pub fn me(&self) -> Rc<Window> {
        self.weak.borrow().upgrade().expect("window alive")
    }

    pub fn connect_location_changed(&self, f: impl Fn(&Window) + 'static) {
        self.location_listeners.borrow_mut().push(Box::new(f));
    }

    // ── Tabs ─────────────────────────────────────────────────────────

    pub fn current_pane(&self) -> Rc<Pane> {
        let panes = self.panes.borrow();
        let idx = self.notebook.current_page().map(|i| i as usize).unwrap_or(0);
        panes.get(idx).or_else(|| panes.first()).cloned().expect("window has a tab")
    }

    pub fn panes(&self) -> Vec<Rc<Pane>> {
        self.panes.borrow().clone()
    }

    pub fn current_location(&self) -> Location {
        self.current_pane().location()
    }

    pub fn add_tab(&self, loc: Location, switch_to: bool) -> Rc<Pane> {
        let pane = Pane::new(self.app.settings.clone(), loc);
        let label_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let icon = gtk::Image::from_gicon(&util::folder_icon(), gtk::IconSize::Menu);
        let label = gtk::Label::new(Some(&pane.location().title()));
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(24);
        let close = gtk::Button::from_icon_name(Some("window-close"), gtk::IconSize::Menu);
        close.set_relief(gtk::ReliefStyle::None);
        close.set_focus_on_click(false);
        gtk::prelude::WidgetExt::set_tooltip_text(&close, Some("Close tab"));
        label_box.pack_start(&icon, false, false, 0);
        label_box.pack_start(&label, true, true, 0);
        label_box.pack_start(&close, false, false, 0);
        label_box.show_all();

        let weak = self.weak.borrow().clone();
        let pw = Rc::downgrade(&pane);
        close.connect_clicked(move |_| {
            if let (Some(w), Some(p)) = (weak.upgrade(), pw.upgrade()) {
                w.close_tab(&p);
            }
        });

        let weak = self.weak.borrow().clone();
        let pw = Rc::downgrade(&pane);
        let lbl = label.clone();
        pane.connect(move |ev| {
            let (Some(w), Some(p)) = (weak.upgrade(), pw.upgrade()) else { return };
            match ev {
                PaneEvent::Location => {
                    super::extensions::pane_location_changed(&w, &p);
                    lbl.set_text(&p.location().title());
                    if Rc::ptr_eq(&p, &w.current_pane()) {
                        w.sync_to_pane();
                    }
                }
                PaneEvent::Selection | PaneEvent::Contents => {
                    if Rc::ptr_eq(&p, &w.current_pane()) {
                        w.update_status();
                        super::extensions::selection_changed(&w);
                    }
                }
                PaneEvent::ContextMenu(ev) => w.show_context_menu(&p, ev.as_ref()),
                PaneEvent::HeaderMenu(ev) => w.show_header_menu(ev),
                PaneEvent::OpenInNewTab(loc) => {
                    w.add_tab(loc.clone(), false);
                }
                PaneEvent::OpenFiles(items) => w.open_items(items),
                PaneEvent::Zoom(step) => w.zoom(*step),
                PaneEvent::Scrolled => super::extensions::pane_scrolled(&w, &p),
                PaneEvent::ClickedEmpty => {
                    w.pathbar.show_crumbs();
                    w.location_entry.set_text(&p.location().display());
                }
                PaneEvent::PasswordNeeded(loc) => super::extensions::ask_archive_password(&w, &p, loc),
                PaneEvent::ShareNotResponding(root) => super::shares::offer_reconnect(&w, root),
            }
            if matches!(ev, PaneEvent::Contents) {
                super::extensions::pane_contents_changed(&w, &p);
            }
        });

        super::extensions::pane_created(self, &pane);
        let page = self.notebook.append_page(&pane.root, Some(&label_box));
        self.notebook.set_tab_reorderable(&pane.root, true);
        self.panes.borrow_mut().push(pane.clone());
        self.notebook.set_show_tabs(self.panes.borrow().len() > 1);
        if switch_to {
            self.notebook.set_current_page(Some(page));
        }
        pane
    }

    pub fn close_tab(&self, pane: &Rc<Pane>) {
        if self.panes.borrow().len() <= 1 {
            self.win.close();
            return;
        }
        if let Some(n) = self.notebook.page_num(&pane.root) {
            self.notebook.remove_page(Some(n));
        }
        self.panes.borrow_mut().retain(|p| !Rc::ptr_eq(p, pane));
        self.notebook.set_show_tabs(self.panes.borrow().len() > 1);
        self.sync_to_pane();
    }

    /// Updates everything that reflects the current tab.
    fn sync_to_pane(&self) {
        if self.panes.borrow().is_empty() {
            return;
        }
        let pane = self.current_pane();
        let loc = pane.location();
        self.win.set_title(&format!("{} - failBrauwser", loc.title()));
        self.pathbar.set_location(&loc);
        let icon = match &loc {
            Location::Archive(_) => util::icon_for_type("application/x-archive"),
            Location::Drives => gio::ThemedIcon::new("drive-harddisk").upcast(),
            _ => util::folder_icon(),
        };
        self.location_entry.set_icon_from_gicon(gtk::EntryIconPosition::Primary, Some(&icon));
        let set = |name: &str, on: bool| {
            if let Some(a) = self.win.lookup_action(name).and_then(|a| a.downcast::<gio::SimpleAction>().ok()) {
                a.set_enabled(on);
            }
        };
        set("back", pane.can_go_back());
        set("forward", pane.can_go_forward());
        set("up", loc.parent().is_some());
        let _ = (&self.back_btn, &self.fwd_btn, &self.up_btn);
        self.update_status();
        super::extensions::selection_changed(self);
        for l in self.location_listeners.borrow().iter() {
            l(self);
        }
    }

    /// Shows a transient message in the status bar (until the next update).
    pub fn set_status(&self, text: &str) {
        let ctx = self.statusbar.context_id("main");
        self.statusbar.remove_all(ctx);
        self.statusbar.push(ctx, text);
    }

    pub fn update_status(&self) {
        super::summary::update(self, false);
        let pane = self.current_pane();
        let ctx = self.statusbar.context_id("main");
        self.statusbar.remove_all(ctx);
        let text = if pane.location() == Location::Drives {
            let n = super::drives::shown_volumes(&pane).len();
            format!("{n} drive{}", if n == 1 { "" } else { "s" })
        } else if pane.is_loading() && pane.item_count() == 0 {
            "Loading…".to_string()
        } else if let Some(err) = pane.error() {
            err
        } else {
            status_text(&pane)
        };
        self.statusbar.push(ctx, &text);
    }

    // ── Opening things ───────────────────────────────────────────────

    pub fn navigate(&self, loc: Location) {
        self.current_pane().navigate(loc);
        self.current_pane().focus_view();
    }

    pub fn open_items(&self, items: &[Item]) {
        // One file no application is set for: if the archive library can read it (a format
        // known by its contents, or by a name that also belongs to other programs), it opens
        // as a folder; otherwise the application chooser comes up as before.
        if let [Item::Fs(e)] = items {
            if gio::AppInfo::default_for_type(&e.content_type, false).is_none() {
                super::archive::enter_or_open(self, e.path.clone());
                return;
            }
        }
        for item in items {
            if let Some(p) = item.path() {
                util::open_with_default(self.win.upcast_ref(), p);
            } else {
                super::extensions::open_non_local(self, item);
            }
        }
    }

    // ── Menus ────────────────────────────────────────────────────────

    fn show_context_menu(&self, pane: &Rc<Pane>, ev: Option<&gdk::EventButton>) {
        let items = pane.selected_items();
        let model = super::extensions::context_menu_model(self, pane, &items);
        let menu = gtk::Menu::from_model(&model);
        menu.set_attach_widget(Some(&self.win));
        menu.show_all();
        match ev {
            Some(ev) => menu.popup_at_pointer(Some(ev)),
            None => {
                let anchor: gtk::Widget = if pane.stack.visible_child_name().as_deref() == Some("icons") {
                    pane.icons.clone().upcast()
                } else {
                    pane.tree.clone().upcast()
                };
                menu.popup_at_widget(&anchor, gdk::Gravity::Center, gdk::Gravity::NorthWest, None);
            }
        }
    }

    fn show_header_menu(&self, ev: &gdk::EventButton) {
        let menu = gtk::Menu::from_model(&actions::columns_menu());
        menu.set_attach_widget(Some(&self.win));
        menu.show_all();
        menu.popup_at_pointer(Some(ev));
    }

    // ── Actions ──────────────────────────────────────────────────────

    pub fn add_action(&self, name: &str, f: impl Fn(&Window) + 'static) -> gio::SimpleAction {
        let a = gio::SimpleAction::new(name, None);
        let weak = self.weak.borrow().clone();
        a.connect_activate(move |_, _| {
            if let Some(w) = weak.upgrade() {
                f(&w);
            }
        });
        self.win.add_action(&a);
        a
    }

    fn install_actions(&self) {
        self.add_action("new-tab", |w| {
            let loc = w.current_location();
            w.add_tab(loc, true);
        });
        self.add_action("close-tab", |w| {
            let p = w.current_pane();
            w.close_tab(&p);
        });
        self.add_action("close", |w| w.win.close());
        self.add_action("back", |w| w.current_pane().go_back());
        self.add_action("forward", |w| w.current_pane().go_forward());
        self.add_action("up", |w| w.current_pane().go_up());
        self.add_action("home", |w| w.navigate(Location::home()));
        self.add_action("drives", |w| w.navigate(Location::Drives));
        self.add_action("zoom-in", |w| w.zoom(1));
        self.add_action("zoom-out", |w| w.zoom(-1));
        self.add_action("zoom-normal", |w| w.zoom(0));
        self.add_action("reload", |w| w.current_pane().reload());
        self.add_action("location", |w| {
            w.pathbar.edit();
        });
        self.add_action("select-all", |w| w.current_pane().select_all());
        self.add_action("next-tab", |w| w.notebook.next_page());
        self.add_action("prev-tab", |w| w.notebook.prev_page());
        self.add_action("open", |w| {
            let p = w.current_pane();
            p.activate(p.selected_items());
        });
        self.add_action("open-in-tab", |w| {
            let p = w.current_pane();
            for i in p.selected_items().iter().filter(|i| i.is_dir_like()) {
                if let Some(l) = p.location().child(&i.display_name()) {
                    w.add_tab(l, false);
                }
            }
        });
        self.add_action("open-with-other", |w| {
            let files: Vec<gio::File> = w.current_pane().selected_items().iter().filter_map(|i| i.path().map(gio::File::for_path)).collect();
            util::choose_application(w.win.upcast_ref(), &files);
        });
        let a = gio::SimpleAction::new("open-with", Some(glib::VariantTy::STRING));
        let weak = self.weak.borrow().clone();
        a.connect_activate(move |_, v| {
            let (Some(w), Some(id)) = (weak.upgrade(), v.and_then(|v| v.get::<String>())) else { return };
            let Some(app) = gio::AppInfo::all().into_iter().find(|a| a.id().as_deref() == Some(id.as_str())) else { return };
            let files: Vec<gio::File> = w.current_pane().selected_items().iter().filter_map(|i| i.path().map(gio::File::for_path)).collect();
            util::launch_with(w.win.upcast_ref(), &app, &files);
        });
        self.win.add_action(&a);

        let s = self.app.settings.clone();
        let app = Rc::downgrade(&self.app);
        self.win.add_action(&toggle_action("show-hidden", s.borrow().show_hidden, move |on| {
            let Some(app) = app.upgrade() else { return };
            app.settings.borrow_mut().show_hidden = on;
            app.save_settings_soon();
            app.for_each_window(|w| {
                w.sync_action_state("show-hidden", on.to_variant());
                for p in w.panes() {
                    p.refilter();
                }
                super::extensions::settings_changed(w);
            });
        }));

        let app = Rc::downgrade(&self.app);
        self.win.add_action(&toggle_action("show-thumbnails", s.borrow().show_thumbnails, move |on| {
            let Some(app) = app.upgrade() else { return };
            app.settings.borrow_mut().show_thumbnails = on;
            app.save_settings_soon();
            app.for_each_window(|w| {
                w.sync_action_state("show-thumbnails", on.to_variant());
                for p in w.panes() {
                    // Off: drop them; on: ask anew.
                    p.refilter();
                }
            });
        }));

        let app = Rc::downgrade(&self.app);
        self.win.add_action(&toggle_action("folders-first", s.borrow().folders_first, move |on| {
            let Some(app) = app.upgrade() else { return };
            app.settings.borrow_mut().folders_first = on;
            app.save_settings_soon();
            app.for_each_window(|w| {
                w.sync_action_state("folders-first", on.to_variant());
                for p in w.panes() {
                    p.apply_folders_first();
                }
            });
        }));

        let app = Rc::downgrade(&self.app);
        let mode = if s.borrow().view_mode == ViewMode::Icons { "icons" } else { "list" };
        let weak = self.weak.borrow().clone();
        self.win.add_action(&radio_action("view-mode", mode, move |m| {
            let (Some(app), Some(w)) = (app.upgrade(), weak.upgrade()) else { return };
            app.settings.borrow_mut().view_mode = if m == "icons" { ViewMode::Icons } else { ViewMode::List };
            app.save_settings_soon();
            for p in w.panes() {
                p.apply_view_mode();
                super::extensions::pane_scrolled(&w, &p);
            }
        }));

        let weak = self.weak.borrow().clone();
        let app = Rc::downgrade(&self.app);
        self.win.add_action(&toggle_action("show-sidebar", s.borrow().sidebar_width > 0, move |on| {
            let (Some(w), Some(app)) = (weak.upgrade(), app.upgrade()) else { return };
            w.side.set_visible(on);
            let mut st = app.settings.borrow_mut();
            if on {
                if st.sidebar_width <= 0 {
                    st.sidebar_width = 220;
                }
                w.paned.set_position(st.sidebar_width);
            } else {
                st.sidebar_width = 0;
            }
            drop(st);
            app.save_settings_soon();
        }));

        let app = Rc::downgrade(&self.app);
        self.win.add_action(&toggle_action("show-tree", s.borrow().show_tree, move |on| {
            let Some(app) = app.upgrade() else { return };
            app.settings.borrow_mut().show_tree = on;
            app.save_settings_soon();
            app.for_each_window(|w| {
                w.sync_action_state("show-tree", on.to_variant());
                super::extensions::settings_changed(w);
            });
        }));

        let app = Rc::downgrade(&self.app);
        self.win.add_action(&radio_action("tree-root", s.borrow().tree_root.as_str(), move |r| {
            let Some(app) = app.upgrade() else { return };
            app.settings.borrow_mut().tree_root = TreeRoot::parse(r).unwrap_or(TreeRoot::Home);
            app.save_settings_soon();
            app.for_each_window(|w| {
                w.sync_action_state("tree-root", r.to_variant());
                super::extensions::settings_changed(w);
            });
        }));

        for id in COLUMN_IDS.iter().filter(|c| **c != "name") {
            let app = Rc::downgrade(&self.app);
            let visible = s.borrow().column_visible(id);
            let name = format!("column-{id}");
            let action_name = name.clone();
            self.win.add_action(&toggle_action(&name, visible, move |on| {
                let Some(app) = app.upgrade() else { return };
                app.settings.borrow_mut().set_column_visible(id, on);
                app.save_settings_soon();
                app.for_each_window(|w| {
                    w.sync_action_state(&action_name, on.to_variant());
                    for p in w.panes() {
                        p.build_columns();
                    }
                    super::extensions::settings_changed(w);
                });
            }));
        }
    }

    /// Zooms the current view mode by `step` levels (0 = normal size), in every window.
    pub fn zoom(&self, step: i32) {
        let flavor_changed = {
            let mut s = self.app.settings.borrow_mut();
            let before = failbrauwser::thumbs::Flavor::for_size(s.icon_size());
            if step == 0 {
                s.zoom_reset();
            } else if !s.zoom(step) {
                return;
            }
            failbrauwser::thumbs::Flavor::for_size(s.icon_size()) != before
        };
        self.app.save_settings_soon();
        self.app.for_each_window(|w| {
            for p in w.panes() {
                // Thumbnails of the other size (128 or 256 px) would be drawn too small, with
                // a wide empty border: fetch them again at the new size.
                if flavor_changed {
                    p.drop_thumbnails();
                    super::thumbnails::reset(&p);
                }
                p.apply_zoom();
                super::extensions::pane_scrolled(w, &p);
            }
        });
    }

    /// Mirrors a global setting into this window's action without re-triggering it.
    pub fn sync_action_state(&self, name: &str, v: glib::Variant) {
        if let Some(a) = self.win.lookup_action(name).and_then(|a| a.downcast::<gio::SimpleAction>().ok()) {
            if a.state().as_ref() != Some(&v) {
                a.set_state(&v);
            }
        }
    }

    // ── Signals ──────────────────────────────────────────────────────

    fn connect_signals(&self) {
        let weak = self.weak.borrow().clone();
        self.pathbar.connect_navigate(move |loc| {
            if let Some(w) = weak.upgrade() {
                w.navigate(loc);
            }
        });
        let weak = self.weak.borrow().clone();
        self.notebook.connect_switch_page(move |_, _, _| {
            if let Some(w) = weak.upgrade() {
                // The switch signal fires before the page counts as current.
                let w2 = Rc::downgrade(&w);
                glib::idle_add_local_once(move || {
                    if let Some(w) = w2.upgrade() {
                        w.sync_to_pane();
                    }
                });
            }
        });
        let weak = self.weak.borrow().clone();
        self.notebook.connect_page_reordered(move |nb, _, _| {
            let Some(w) = weak.upgrade() else { return };
            let mut panes = w.panes.borrow_mut();
            panes.sort_by_key(|p| nb.page_num(&p.root).unwrap_or(u32::MAX));
        });

        let weak = self.weak.borrow().clone();
        self.location_entry.connect_activate(move |e| {
            let Some(w) = weak.upgrade() else { return };
            let text = e.text().to_string();
            if util::is_remote_uri(&text) {
                super::shares::open_remote(&w, &text, false);
                return;
            }
            match Location::parse(&text, failbrauwser::archive::is_browsable_name) {
                Some(loc) => w.navigate(loc),
                None => {
                    e.error_bell();
                    util::show_error(&w.win, "Location not found", &format!("“{text}” does not exist."));
                }
            }
        });
        let weak = self.weak.borrow().clone();
        self.location_entry.connect_key_press_event(move |e, ev| {
            if ev.keyval() == gdk::keys::constants::Escape {
                if let Some(w) = weak.upgrade() {
                    e.set_text(&w.current_location().display());
                    w.current_pane().focus_view();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });

        // Keys go to a focused text entry before window shortcuts, so Delete, Ctrl+C or
        // Ctrl+A edit text there instead of acting on files.
        self.win.connect_key_press_event(|win, ev| {
            if let Some(focus) = gtk::prelude::GtkWindowExt::focused_widget(win) {
                if focus.is::<gtk::Entry>() && win.propagate_key_event(ev) {
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });

        let weak = self.weak.borrow().clone();
        self.win.connect_delete_event(move |win, _| {
            if let Some(w) = weak.upgrade() {
                let (width, height) = win.size();
                let mut s = w.app.settings.borrow_mut();
                s.window_width = width;
                s.window_height = height;
                if w.side.is_visible() {
                    s.sidebar_width = w.paned.position();
                }
            }
            glib::Propagation::Proceed
        });
        let weak = self.weak.borrow().clone();
        self.win.connect_destroy(move |_| {
            if let Some(w) = weak.upgrade() {
                // Break the Rc cycles through the panes so everything is freed.
                w.panes.borrow_mut().clear();
                w.location_listeners.borrow_mut().clear();
                let app = w.app.clone();
                app.window_closed(&w);
            }
        });
    }
}

/// "12 items, free space: 3.1 GB" or a summary of the selection.
fn status_text(pane: &Pane) -> String {
    let sel = pane.selected_items();
    match sel.len() {
        0 => {
            let n = pane.item_count();
            let mut s = format!("{n} item{}", if n == 1 { "" } else { "s" });
            if let Location::Archive(a) = pane.location() {
                if let Some(x) = failbrauwser::archive::vfs::Vfs::global().cached(&a) {
                    let ro = match (&x.readonly_reason, x.writable) {
                        (Some(why), _) => format!(" (read-only: {why})"),
                        (None, false) => " (read-only)".to_string(),
                        _ => String::new(),
                    };
                    s.push_str(&format!(" — {} archive{ro}", x.format));
                }
            }
            if let Some(dir) = pane.location().local_path() {
                // On a network share statvfs can hang: read in the background, shown when known.
                let free = if failbrauwser::ops::gvfs::is_gvfs_path(dir) { failbrauwser::ops::gvfs::free_space_cached(dir, free_space) } else { free_space(dir) };
                if let Some(free) = free {
                    s.push_str(&format!(", free space: {}", human_size(free)));
                }
            }
            s
        }
        1 if matches!(sel[0], Item::Trash(..)) => {
            let Item::Trash(t, e) = &sel[0] else { unreachable!() };
            super::trash::status_text(t, e.size, e.is_dir_like())
        }
        1 => {
            let i = &sel[0];
            let t = util::type_description(&i.content_type());
            if i.is_dir_like() {
                format!("“{}” {}", i.display_name(), t)
            } else {
                format!("“{}” ({}) {}", i.display_name(), human_size(i.size()), t)
            }
        }
        n => {
            let files: Vec<&Item> = sel.iter().filter(|i| !i.is_dir_like()).collect();
            let bytes: u64 = files.iter().map(|i| i.size()).sum();
            if files.is_empty() {
                format!("{n} items selected")
            } else {
                format!("{n} items selected ({})", human_size(bytes))
            }
        }
    }
}

pub fn free_space(dir: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}
