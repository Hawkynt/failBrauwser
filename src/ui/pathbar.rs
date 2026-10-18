//! The location bar: a button for every step of the path, and a text field for typing a
//! location. Clicking right of the last button (or Ctrl+L) switches to the text field;
//! Enter, Escape or clicking elsewhere switches back to the buttons.
//!
//! The path always starts at `/`; the step you are in is shown pressed.

use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

pub struct PathBar {
    pub root: gtk::Stack,
    pub entry: gtk::Entry,
    crumbs: gtk::Box,
    scroller: gtk::ScrolledWindow,
    current: RefCell<Option<Location>>,
    /// Set while the buttons are rebuilt, so toggling the current one does not navigate.
    rebuilding: std::cell::Cell<bool>,
    navigate: RefCell<Option<Box<dyn Fn(Location)>>>,
    weak: RefCell<Weak<PathBar>>,
}

/// One button: its label, icon and target.
#[derive(Debug, Clone, PartialEq)]
pub struct Crumb {
    pub label: String,
    pub icon: Option<&'static str>,
    pub location: Location,
}

/// The steps from `/` down to `loc`.
pub fn crumbs_for(loc: &Location) -> Vec<Crumb> {
    if *loc == Location::Trash {
        return vec![Crumb { label: "Trash".into(), icon: Some("user-trash-symbolic"), location: Location::Trash }];
    }
    if *loc == Location::Drives {
        return vec![Crumb { label: "Drives".into(), icon: Some("drive-harddisk-symbolic"), location: Location::Drives }];
    }
    let mut chain = vec![loc.clone()];
    let mut cur = loc.clone();
    while let Some(p) = cur.parent() {
        chain.push(p.clone());
        cur = p;
    }
    chain.reverse();
    // On a phone the path starts at the phone, not at gvfsd-fuse's folder.
    let device = loc.nearest_dir().and_then(|d| failbrauwser::devices::named_root(&d));
    if let Some((root, ..)) = &device {
        if let Some(i) = chain.iter().position(|l| matches!(l, Location::Dir(p) if p == root)) {
            chain.drain(..i);
        }
    }
    chain
        .into_iter()
        .map(|l| {
            if let (Location::Dir(p), Some((root, name, kind))) = (&l, &device) {
                if p == root {
                    return Crumb { label: name.clone(), icon: Some(kind.symbolic_icon()), location: l };
                }
            }
            let (label, icon) = match &l {
                Location::Dir(p) if *p == glib::home_dir() => (l.title(), Some("user-home-symbolic")),
                Location::Dir(p) if p.parent().is_none() => ("/".to_string(), None),
                Location::Archive(a) if a.inner.is_empty() => (l.title(), Some("package-x-generic-symbolic")),
                _ => (l.title(), None),
            };
            Crumb { label, icon, location: l }
        })
        .collect()
}

impl PathBar {
    pub fn new() -> Rc<PathBar> {
        let entry = gtk::Entry::new();
        entry.set_hexpand(true);
        entry.set_input_purpose(gtk::InputPurpose::Url);

        let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        crumbs.style_context().add_class("linked");
        let scroller = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        // Scrollable without a scrollbar: long paths keep their end in view.
        scroller.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        scroller.set_propagate_natural_width(true);
        scroller.set_shadow_type(gtk::ShadowType::None);
        scroller.add(&crumbs);
        // The empty space after the last button opens the text field.
        let filler = gtk::EventBox::new();
        filler.set_hexpand(true);
        // Always some room to click, even when a long path fills the bar.
        filler.set_size_request(40, -1);
        filler.set_tooltip_text(Some("Click to type a location"));
        // Where typing starts shows the text cursor.
        filler.connect_realize(|f| {
            if let Some(win) = f.window() {
                win.set_cursor(gtk::gdk::Cursor::from_name(&f.display(), "text").as_ref());
            }
        });
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.pack_start(&scroller, false, true, 0);
        row.pack_start(&filler, true, true, 0);
        // Framed like a text field: buttons inside, room to click after them.
        row.style_context().add_class("fb-pathbar");

        let root = gtk::Stack::new();
        root.set_hexpand(true);
        root.add_named(&row, "crumbs");
        root.add_named(&entry, "entry");
        root.set_visible_child_name("crumbs");
        root.show_all();

        let bar = Rc::new(PathBar {
            root,
            entry,
            crumbs,
            scroller,
            current: RefCell::new(None),
            rebuilding: std::cell::Cell::new(false),
            navigate: RefCell::new(None),
            weak: RefCell::new(Weak::new()),
        });
        *bar.weak.borrow_mut() = Rc::downgrade(&bar);

        let weak = Rc::downgrade(&bar);
        filler.connect_button_press_event(move |_, ev| {
            if ev.button() == 1 {
                if let Some(b) = weak.upgrade() {
                    b.edit();
                }
            }
            glib::Propagation::Stop
        });
        let weak = Rc::downgrade(&bar);
        bar.entry.connect_focus_out_event(move |_, _| {
            if let Some(b) = weak.upgrade() {
                b.show_crumbs();
            }
            glib::Propagation::Proceed
        });
        // The last step always stays in view: whenever the path or the room for it
        // changes, scroll to the end.
        bar.scroller.hadjustment().connect_changed(|adj| {
            adj.set_value(adj.upper() - adj.page_size());
        });
        bar
    }

    pub fn connect_navigate(&self, f: impl Fn(Location) + 'static) {
        *self.navigate.borrow_mut() = Some(Box::new(f));
    }

    pub fn is_editing(&self) -> bool {
        self.root.visible_child_name().as_deref() == Some("entry")
    }

    /// Switches to the text field with the whole location selected.
    pub fn edit(&self) {
        if let Some(loc) = self.current.borrow().as_ref() {
            self.entry.set_text(&loc.display());
        }
        self.root.set_visible_child_name("entry");
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
    }

    pub fn show_crumbs(&self) {
        self.root.set_visible_child_name("crumbs");
    }

    /// The labels of the buttons shown (tests).
    pub fn labels(&self) -> Vec<String> {
        self.crumbs
            .children()
            .iter()
            .filter(|c| c.is::<gtk::Button>())
            .map(|c| c.widget_name().to_string())
            .collect()
    }

    /// The last button is fully inside the visible part of the bar (tests).
    pub fn last_visible(&self) -> bool {
        let Some(last) = self.crumbs.children().last().cloned() else { return false };
        let adj = self.scroller.hadjustment();
        let a = last.allocation();
        a.x() as f64 >= adj.value() - 0.5 && (a.x() + a.width()) as f64 <= adj.value() + adj.page_size() + 0.5
    }

    /// Clicks the button with this label (tests).
    pub fn click(&self, label: &str) -> bool {
        for c in self.crumbs.children() {
            if let Some(b) = c.downcast_ref::<gtk::ToggleButton>() {
                if b.widget_name() == label {
                    b.clicked();
                    return true;
                }
            }
        }
        false
    }

    pub fn set_location(&self, loc: &Location) {
        if !self.entry.has_focus() {
            self.entry.set_text(&loc.display());
            self.show_crumbs();
        }
        *self.current.borrow_mut() = Some(loc.clone());
        self.rebuild(&crumbs_for(loc), loc);
    }

    fn rebuild(&self, crumbs: &[Crumb], current: &Location) {
        self.rebuilding.set(true);
        for c in self.crumbs.children() {
            self.crumbs.remove(&c);
        }
        for crumb in crumbs {
            // The step you are in is shown pressed.
            let b = gtk::ToggleButton::new();
            b.set_active(crumb.location == *current);
            b.set_focus_on_click(false);
            // Names identify buttons in tests.
            b.set_widget_name(&crumb.label);
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            if let Some(icon) = crumb.icon {
                content.pack_start(&gtk::Image::from_gicon(&gio::ThemedIcon::new(icon), gtk::IconSize::Menu), false, false, 0);
            }
            if !crumb.label.is_empty() {
                let label = gtk::Label::new(None);
                let text = glib::markup_escape_text(&crumb.label);
                // The current step in bold, like other path bars.
                if crumb.location == *current {
                    label.set_markup(&format!("<b>{text}</b>"));
                } else {
                    label.set_text(&crumb.label);
                }
                // Full names up to 24 characters: a long path scrolls instead of squeezing
                // every step into "th…er".
                if crumb.label.chars().count() > 24 {
                    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                    label.set_width_chars(24);
                    label.set_max_width_chars(24);
                }
                content.pack_start(&label, false, false, 0);
            }
            b.add(&content);
            b.set_tooltip_text(Some(&crumb.location.display()));
            let target = crumb.location.clone();
            let weak = self.weak.borrow().clone();
            b.connect_toggled(move |b| {
                let Some(bar) = weak.upgrade() else { return };
                if bar.rebuilding.get() {
                    return;
                }
                if bar.current.borrow().as_ref() == Some(&target) {
                    // Clicking the current step keeps it pressed.
                    bar.rebuilding.set(true);
                    b.set_active(true);
                    bar.rebuilding.set(false);
                    return;
                }
                if let Some(f) = bar.navigate.borrow().as_ref() {
                    f(target.clone());
                }
            });
            self.crumbs.pack_start(&b, false, false, 0);
        }
        self.crumbs.show_all();
        self.rebuilding.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use failbrauwser::location::ArchiveLoc;
    use std::path::PathBuf;

    #[test]
    fn paths_start_at_the_root_even_below_home() {
        let loc = Location::Dir(glib::home_dir().join("Music/Albums"));
        let c = crumbs_for(&loc);
        assert_eq!(c[0].label, "/");
        assert_eq!(c[0].location, Location::Dir(PathBuf::from("/")));
        let home = c.iter().find(|x| x.location == Location::home()).unwrap();
        assert_eq!(home.icon, Some("user-home-symbolic"));
        assert_eq!(c.last().unwrap().label, "Albums");
    }

    #[test]
    fn other_paths_start_at_the_root() {
        let c = crumbs_for(&Location::Dir(PathBuf::from("/usr/share")));
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].label, "/");
        assert_eq!(c[2].label, "share");
    }

    #[test]
    fn archives_continue_the_path() {
        let a = ArchiveLoc::root(PathBuf::from("/data/x.zip")).enter_nested("in/y.tar").with_inner("docs");
        let c = crumbs_for(&Location::Archive(a));
        let labels: Vec<_> = c.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["/", "data", "x.zip", "in", "y.tar", "docs"]);
        assert_eq!(c[2].icon, Some("package-x-generic-symbolic"));
        assert_eq!(crumbs_for(&Location::Drives)[0].label, "Drives");
    }

    #[test]
    fn paths_on_a_phone_start_at_the_phone() {
        let root = failbrauwser::ops::gvfs::fuse_root().join("mtp:host=Google_Pixel_7_28041FDH2000ZA");
        let c = crumbs_for(&Location::Dir(root.join("Internal shared storage/DCIM")));
        let labels: Vec<_> = c.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["Google Pixel 7", "Internal shared storage", "DCIM"]);
        assert_eq!(c[0].icon, Some("phone-symbolic"));
        assert_eq!(c[0].location, Location::Dir(root.clone()));
        assert_eq!(Location::Dir(root).title(), "Google Pixel 7");
    }
}
