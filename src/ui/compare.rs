//! Tools > Compare Folders: two folders side by side, however deep, with what is only on
//! one side, newer on one side or different; the missing and newer ones are copied across
//! with a click (nothing is ever deleted).

use super::jobs::Outcome;
use super::window::Window;
use failbrauwser::fs::compare::{self, Row, State};
use failbrauwser::fs::format::{human_size, human_time};
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const COL_PATH: u32 = 0;
const COL_LEFT: u32 = 1;
const COL_STATE: u32 = 2;
const COL_RIGHT: u32 = 3;
/// Index into the rows of the last comparison.
const COL_INDEX: u32 = 4;

pub struct Compare {
    pub window: gtk::Window,
    left: gtk::FileChooserButton,
    right: gtk::FileChooserButton,
    by_content: gtk::CheckButton,
    show_same: gtk::CheckButton,
    store: gtk::ListStore,
    tree: gtk::TreeView,
    status: gtk::Label,
    spinner: gtk::Spinner,
    to_right: gtk::Button,
    to_left: gtk::Button,
    rows: RefCell<Vec<Row>>,
    /// The folders the rows are about.
    compared: RefCell<Option<(PathBuf, PathBuf)>>,
    cancel: RefCell<Arc<AtomicBool>>,
    main: Weak<Window>,
    weak: RefCell<Weak<Compare>>,
}

thread_local! {
    static OPEN: RefCell<Vec<Rc<Compare>>> = const { RefCell::new(Vec::new()) };
}

/// The compare window shown (tests).
pub fn current() -> Option<Rc<Compare>> {
    OPEN.with(|o| o.borrow().iter().rev().find(|c| c.window.is_visible()).cloned())
}

fn state_text(s: State) -> &'static str {
    match s {
        State::OnlyLeft => "only left",
        State::OnlyRight => "only right",
        State::Same => "identical",
        State::LeftNewer => "left newer",
        State::RightNewer => "right newer",
        State::Differs => "different",
    }
}

fn arrow(s: State) -> &'static str {
    match s {
        State::OnlyLeft | State::LeftNewer => "⇢",
        State::OnlyRight | State::RightNewer => "⇠",
        State::Same => "=",
        State::Differs => "≠",
    }
}

/// Opens the compare window for two folders (`right` may be unknown yet).
pub fn show(w: &Window, left: Option<PathBuf>, right: Option<PathBuf>) {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Compare Folders");
    window.set_transient_for(Some(&w.win));
    window.set_default_size(820, 560);
    let chooser = |title: &str, p: &Option<PathBuf>| {
        let c = gtk::FileChooserButton::new(title, gtk::FileChooserAction::SelectFolder);
        if let Some(p) = p {
            c.set_filename(p);
        }
        c.set_hexpand(true);
        c
    };
    let left_btn = chooser("Left folder", &left);
    let right_btn = chooser("Right folder", &right);
    let by_content = gtk::CheckButton::with_mnemonic("Compare _contents");
    by_content.set_tooltip_text(Some("Also compare files of the same size byte by byte (slower)"));
    let show_same = gtk::CheckButton::with_mnemonic("Show _identical");
    let go = gtk::Button::with_mnemonic("C_ompare");
    let spinner = gtk::Spinner::new();
    let top = gtk::Grid::new();
    top.set_column_spacing(8);
    top.set_row_spacing(6);
    top.attach(&gtk::Label::new(Some("Left:")), 0, 0, 1, 1);
    top.attach(&left_btn, 1, 0, 1, 1);
    top.attach(&gtk::Label::new(Some("Right:")), 2, 0, 1, 1);
    top.attach(&right_btn, 3, 0, 1, 1);
    let opts = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    opts.pack_start(&by_content, false, false, 0);
    opts.pack_start(&show_same, false, false, 0);
    opts.pack_end(&go, false, false, 0);
    opts.pack_end(&spinner, false, false, 0);
    top.attach(&opts, 0, 1, 4, 1);

    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::U32]);
    let tree = gtk::TreeView::with_model(&store);
    tree.selection().set_mode(gtk::SelectionMode::Multiple);
    for (title, col, expand) in [("Name", COL_PATH, true), ("Left", COL_LEFT, false), ("", COL_STATE, false), ("Right", COL_RIGHT, false)] {
        let cell = gtk::CellRendererText::new();
        // Names are shortened; sizes, dates and states keep their width.
        if expand {
            cell.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        }
        let c = gtk::TreeViewColumn::new();
        c.set_title(title);
        TreeViewColumnExt::pack_start(&c, &cell, true);
        TreeViewColumnExt::add_attribute(&c, &cell, "text", col as i32);
        c.set_resizable(true);
        c.set_expand(expand);
        tree.append_column(&c);
    }
    let scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroll.set_vexpand(true);
    scroll.set_shadow_type(gtk::ShadowType::In);
    scroll.add(&tree);
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_hexpand(true);
    let to_left = gtk::Button::with_mnemonic("⇠ Copy to _Left");
    let to_right = gtk::Button::with_mnemonic("Copy to _Right ⇢");
    to_left.set_tooltip_text(Some("Copies the selected rows (none selected: all of them) that are missing or older on the left"));
    to_right.set_tooltip_text(Some("Copies the selected rows (none selected: all of them) that are missing or older on the right"));
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bottom.pack_start(&status, true, true, 0);
    bottom.pack_end(&to_right, false, false, 0);
    bottom.pack_end(&to_left, false, false, 0);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_border_width(12);
    root.pack_start(&top, false, false, 0);
    root.pack_start(&scroll, true, true, 0);
    root.pack_start(&bottom, false, false, 0);
    window.add(&root);

    let c = Rc::new(Compare {
        window: window.clone(),
        left: left_btn,
        right: right_btn,
        by_content,
        show_same,
        store,
        tree,
        status,
        spinner,
        to_right,
        to_left,
        rows: RefCell::new(Vec::new()),
        compared: RefCell::new(None),
        cancel: RefCell::new(Arc::new(AtomicBool::new(false))),
        main: Rc::downgrade(&w.me()),
        weak: RefCell::new(Weak::new()),
    });
    *c.weak.borrow_mut() = Rc::downgrade(&c);
    let weak = Rc::downgrade(&c);
    go.connect_clicked(move |_| {
        if let Some(c) = weak.upgrade() {
            c.run();
        }
    });
    let weak = Rc::downgrade(&c);
    c.show_same.connect_toggled(move |_| {
        if let Some(c) = weak.upgrade() {
            c.fill();
        }
    });
    for (btn, to_right) in [(&c.to_right, true), (&c.to_left, false)] {
        let weak = Rc::downgrade(&c);
        btn.connect_clicked(move |_| {
            if let Some(c) = weak.upgrade() {
                c.copy(to_right);
            }
        });
    }
    let weak = Rc::downgrade(&c);
    c.tree.selection().connect_changed(move |_| {
        if let Some(c) = weak.upgrade() {
            c.update_buttons();
        }
    });
    let weak = Rc::downgrade(&c);
    window.connect_destroy(move |_| {
        if let Some(c) = weak.upgrade() {
            c.cancel.borrow().store(true, Ordering::Relaxed);
            OPEN.with(|o| o.borrow_mut().retain(|x| !Rc::ptr_eq(x, &c)));
        }
    });
    OPEN.with(|o| o.borrow_mut().push(c.clone()));
    window.show_all();
    c.update_buttons();
    if left.is_some() && right.is_some() {
        c.run();
    }
}

impl Compare {
    fn me(&self) -> Rc<Compare> {
        self.weak.borrow().upgrade().expect("compare window alive")
    }

    /// Compares the two chosen folders on a worker thread.
    pub fn run(&self) {
        let (Some(l), Some(r)) = (self.left.filename(), self.right.filename()) else {
            self.status.set_text("Choose a folder on each side.");
            return;
        };
        self.cancel.borrow().store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        *self.cancel.borrow_mut() = cancel.clone();
        self.spinner.start();
        self.status.set_text("Comparing…");
        let by_content = self.by_content.is_active();
        let me = self.me();
        let (l2, r2) = (l.clone(), r.clone());
        glib::spawn_future_local(async move {
            let res = gtk::gio::spawn_blocking(move || compare::compare(&l2, &r2, by_content, &cancel)).await;
            me.spinner.stop();
            match res {
                Ok(Ok(rows)) => {
                    *me.rows.borrow_mut() = rows;
                    *me.compared.borrow_mut() = Some((l, r));
                    me.fill();
                }
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Ok(Err(e)) => me.status.set_text(&format!("Cannot compare: {e}")),
                Err(_) => {}
            }
        });
    }

    fn fill(&self) {
        self.store.clear();
        let now = glib::real_time() / 1_000_000;
        let side = |m: &Option<compare::Meta>| match m {
            None => String::new(),
            Some(m) if m.dir => "folder".into(),
            Some(m) => format!("{} · {}", human_size(m.size), human_time(m.mtime, now)),
        };
        let rows = self.rows.borrow();
        let show_same = self.show_same.is_active();
        let mut counts = [0usize; 6];
        for (i, r) in rows.iter().enumerate() {
            counts[r.state as usize] += 1;
            if r.state == State::Same && !show_same {
                continue;
            }
            // A relative path reads with `/` on every platform: the components joined are
            // the same text Linux already shows, and a name holding a backslash stays whole.
            let rel = r.rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
            let path = if r.left.or(r.right).is_some_and(|m| m.dir) { format!("{rel}/") } else { rel };
            let state = format!("{} {}", arrow(r.state), state_text(r.state));
            self.store.insert_with_values(None, &[(COL_PATH, &path), (COL_LEFT, &side(&r.left)), (COL_STATE, &state), (COL_RIGHT, &side(&r.right)), (COL_INDEX, &(i as u32))]);
        }
        let differ = rows.len() - counts[State::Same as usize];
        self.status.set_text(&if differ == 0 {
            format!("The folders are the same ({} identical).", counts[State::Same as usize])
        } else {
            format!(
                "{differ} differences: {} only left, {} only right, {} newer left, {} newer right, {} different; {} identical.",
                counts[State::OnlyLeft as usize],
                counts[State::OnlyRight as usize],
                counts[State::LeftNewer as usize],
                counts[State::RightNewer as usize],
                counts[State::Differs as usize],
                counts[State::Same as usize]
            )
        });
        drop(rows);
        self.update_buttons();
    }

    /// The rows the copy buttons act on: the selected ones, else all shown.
    fn chosen(&self) -> Vec<Row> {
        let rows = self.rows.borrow();
        let (paths, model) = self.tree.selection().selected_rows();
        let idx: Vec<usize> = if paths.is_empty() {
            let mut v = Vec::new();
            model.foreach(|m, _, it| {
                v.push(m.value(it, COL_INDEX as i32).get::<u32>().unwrap_or(0) as usize);
                false
            });
            v
        } else {
            paths.iter().filter_map(|p| model.iter(p)).map(|it| model.value(&it, COL_INDEX as i32).get::<u32>().unwrap_or(0) as usize).collect()
        };
        idx.into_iter().filter_map(|i| rows.get(i).cloned()).collect()
    }

    fn update_buttons(&self) {
        let rows = self.chosen();
        self.to_right.set_sensitive(rows.iter().any(|r| compare::copies(r, true)));
        self.to_left.set_sensitive(rows.iter().any(|r| compare::copies(r, false)));
    }

    /// Copies the chosen rows that are missing or older on the other side, then compares again.
    fn copy(&self, to_right: bool) {
        let Some((l, r)) = self.compared.borrow().clone() else { return };
        let Some(w) = self.main.upgrade() else { return };
        let rels: Vec<PathBuf> = self.chosen().into_iter().filter(|x| compare::copies(x, to_right)).map(|x| x.rel).collect();
        if rels.is_empty() {
            return;
        }
        let (from, to) = if to_right { (l, r) } else { (r, l) };
        let title = format!("Copying {} to “{}”", if rels.len() == 1 { "1 item".to_string() } else { format!("{} items", rels.len()) }, to.file_name().unwrap_or_default().to_string_lossy());
        let me = self.me();
        w.app.jobs.start(w.win.upcast_ref(), title, move |ctx| compare::copy_across(ctx, &from, &to, &rels), move |outcome| {
            if !matches!(outcome, Outcome::Cancelled) {
                me.run();
            }
        });
    }

    /// The rows shown as "path state" (tests).
    pub fn row_texts(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.store.foreach(|m, _, it| {
            let p: String = m.value(it, COL_PATH as i32).get().unwrap_or_default();
            let s: String = m.value(it, COL_STATE as i32).get().unwrap_or_default();
            out.push(format!("{p} {s}"));
            false
        });
        out
    }

    pub fn is_busy(&self) -> bool {
        self.spinner.property::<bool>("active")
    }

    pub fn set_show_same(&self, on: bool) {
        self.show_same.set_active(on);
    }

    pub fn click_copy(&self, to_right: bool) {
        self.copy(to_right);
    }
}
