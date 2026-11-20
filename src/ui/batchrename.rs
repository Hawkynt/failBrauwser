//! Renaming several files at once (F2 with more than one selected): find and replace, a
//! name template with a counter and the date, the letter case, and a preview of every new
//! name with what is wrong with it. Taken back with Ctrl+Z like any rename.

use super::jobs::Outcome;
use super::model::Item;
use super::window::Window;
use failbrauwser::ops::batchrename::{self, Case, Rule, Source};
use failbrauwser::ops::undo::{Step, Undoable};
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

/// Preview columns: old name, new name, what is wrong (markup), new name changed.
const COL_OLD: u32 = 0;
const COL_NEW: u32 = 1;
const COL_PROBLEM: u32 = 2;

struct Dialog {
    dialog: gtk::Dialog,
    find: gtk::Entry,
    replace: gtk::Entry,
    regex: gtk::CheckButton,
    ignore_case: gtk::CheckButton,
    template: gtk::Entry,
    start: gtk::SpinButton,
    case: gtk::ComboBoxText,
    store: gtk::ListStore,
    status: gtk::Label,
    files: Vec<(PathBuf, Source)>,
    /// The names in each folder when the dialog opened.
    taken: HashMap<PathBuf, HashSet<String>>,
    /// The renames the current settings make, if they can be done.
    ready: RefCell<Option<Vec<(PathBuf, String)>>>,
}

pub fn show(w: &Window, items: Vec<Item>) {
    let files: Vec<(PathBuf, Source)> = items
        .iter()
        .filter_map(|i| match i {
            Item::Fs(e) | Item::Found(e, _) => Some((e.path.clone(), Source { name: i.display_name(), mtime: e.mtime })),
            _ => None,
        })
        .collect();
    if files.is_empty() {
        return;
    }
    let mut taken: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    for (p, _) in &files {
        let dir = batchrename::folder_of(p);
        taken.entry(dir.clone()).or_insert_with(|| {
            std::fs::read_dir(&dir).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
        });
    }

    // Built like the Properties window: a window of its own with the title in its title bar, the
    // selection's icon and count at the top, labelled rows, and the buttons at the bottom right.
    let dialog = gtk::Dialog::with_buttons(
        Some(&format!("Rename {} Items", files.len())),
        Some(&w.win),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[("_Cancel", gtk::ResponseType::Cancel), ("_Rename", gtk::ResponseType::Ok)],
    );
    dialog.set_default_response(gtk::ResponseType::Ok);
    let area = dialog.content_area();
    area.set_border_width(12);
    area.set_spacing(12);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    header.pack_start(&gtk::Image::from_icon_name(Some("edit-select-all"), gtk::IconSize::Dialog), false, false, 0);
    let heading = gtk::Label::new(None);
    heading.set_markup(&format!("<big><b>{} items</b></big>", files.len()));
    heading.set_xalign(0.0);
    header.pack_start(&heading, true, true, 0);

    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(12);
    let label = |text: &str| {
        let l = gtk::Label::with_mnemonic(text);
        l.set_xalign(1.0);
        l.style_context().add_class("dim-label");
        l
    };
    let entry = |name: &str, text: &str| {
        let e = gtk::Entry::new();
        e.set_widget_name(name);
        e.set_text(text);
        e.set_hexpand(true);
        e.set_activates_default(true);
        e
    };
    let find = entry("batch-find", "");
    let replace = entry("batch-replace", "");
    let regex = gtk::CheckButton::with_mnemonic("Regular e_xpression");
    regex.set_widget_name("batch-regex");
    let ignore_case = gtk::CheckButton::with_mnemonic("_Ignore case");
    ignore_case.set_widget_name("batch-ignore-case");
    let template = entry("batch-template", "{name}{ext}");
    template.set_tooltip_text(Some("{name} the name, {ext} its extension, {n} a number ({n:03}: 001, 002, …), {date} the day it was modified"));
    let start = gtk::SpinButton::with_range(0.0, 1_000_000.0, 1.0);
    start.set_widget_name("batch-start");
    start.set_value(1.0);
    let case = gtk::ComboBoxText::new();
    case.set_widget_name("batch-case");
    for (id, text) in [("keep", "Keep"), ("lower", "lowercase"), ("upper", "UPPERCASE"), ("title", "Title Case")] {
        case.append(Some(id), text);
    }
    case.set_active_id(Some("keep"));
    let options = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    options.pack_start(&regex, false, false, 0);
    options.pack_start(&ignore_case, false, false, 0);
    let numbers = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    numbers.pack_start(&start, false, false, 0);
    numbers.pack_start(&label("Ca_se:"), false, false, 0);
    numbers.pack_start(&case, false, false, 0);
    let hint = gtk::Label::new(Some("{name}  {ext}  {n}  {n:03}  {date}"));
    hint.set_xalign(0.0);
    hint.style_context().add_class("dim-label");
    let rows: [(&str, gtk::Widget); 6] = [
        ("_Find:", find.clone().upcast()),
        ("Replace _with:", replace.clone().upcast()),
        ("", options.upcast()),
        ("New _name:", template.clone().upcast()),
        ("", hint.upcast()),
        ("Number _from:", numbers.upcast()),
    ];
    for (y, (text, widget)) in rows.into_iter().enumerate() {
        if !text.is_empty() {
            let l = label(text);
            l.set_mnemonic_widget(Some(&widget));
            grid.attach(&l, 0, y as i32, 1, 1);
        }
        grid.attach(&widget, 1, y as i32, 1, 1);
    }

    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let tree = gtk::TreeView::with_model(&store);
    for (title, col, markup) in [("Name", COL_OLD, false), ("New Name", COL_NEW, false), ("", COL_PROBLEM, true)] {
        let cell = gtk::CellRendererText::new();
        cell.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let c = gtk::TreeViewColumn::new();
        c.set_title(title);
        TreeViewColumnExt::pack_start(&c, &cell, true);
        TreeViewColumnExt::add_attribute(&c, &cell, if markup { "markup" } else { "text" }, col as i32);
        c.set_resizable(true);
        c.set_expand(!markup);
        tree.append_column(&c);
    }
    let scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_min_content_height(220);
    scroll.set_min_content_width(560);
    scroll.set_shadow_type(gtk::ShadowType::In);
    scroll.add(&tree);
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_widget_name("batch-status");

    area.pack_start(&header, false, false, 0);
    area.pack_start(&grid, false, false, 0);
    area.pack_start(&scroll, true, true, 0);
    area.pack_start(&status, false, false, 0);

    let d = Rc::new(Dialog { dialog: dialog.clone(), find, replace, regex, ignore_case, template, start, case, store, status, files, taken, ready: RefCell::new(None) });
    let update = {
        let d = Rc::downgrade(&d);
        move || {
            if let Some(d) = d.upgrade() {
                d.update();
            }
        }
    };
    for e in [&d.find, &d.replace, &d.template] {
        let u = update.clone();
        e.connect_changed(move |_| u());
    }
    for c in [&d.regex, &d.ignore_case] {
        let u = update.clone();
        c.connect_toggled(move |_| u());
    }
    let u = update.clone();
    d.start.connect_value_changed(move |_| u());
    let u = update.clone();
    d.case.connect_changed(move |_| u());
    d.update();

    let weak = Rc::downgrade(&w.me());
    let d2 = d.clone();
    let answered = std::cell::Cell::new(false);
    dialog.connect_response(move |dlg, resp| {
        if answered.replace(true) {
            return;
        }
        let renames = d2.ready.borrow().clone();
        // Not close(): see util::ask_text.
        unsafe { dlg.destroy() };
        if resp != gtk::ResponseType::Ok {
            return;
        }
        if let (Some(w), Some(renames)) = (weak.upgrade(), renames) {
            run(&w, renames);
        }
    });
    dialog.show_all();
    d.find.grab_focus();
}

impl Dialog {
    fn rule(&self) -> Rule {
        Rule {
            find: self.find.text().to_string(),
            replace: self.replace.text().to_string(),
            regex: self.regex.is_active(),
            ignore_case: self.ignore_case.is_active(),
            template: self.template.text().to_string(),
            start: self.start.value() as i64,
            case: match self.case.active_id().as_deref() {
                Some("lower") => Case::Lower,
                Some("upper") => Case::Upper,
                Some("title") => Case::Title,
                _ => Case::Keep,
            },
        }
    }

    /// Fills the preview and decides whether Rename may be pressed.
    fn update(&self) {
        self.store.clear();
        let sources: Vec<Source> = self.files.iter().map(|(_, s)| s.clone()).collect();
        let new = match batchrename::plan(&self.rule(), &sources) {
            Ok(n) => n,
            Err(e) => {
                self.status.set_markup(&format!("<b>The search pattern is not valid:</b> {}", glib::markup_escape_text(&e)));
                self.set_ready(None);
                return;
            }
        };
        // Checked folder by folder (search results come from many).
        let mut problems: Vec<Option<String>> = vec![None; self.files.len()];
        let mut by_dir: HashMap<PathBuf, Vec<usize>> = HashMap::new();
        for (i, (p, _)) in self.files.iter().enumerate() {
            by_dir.entry(batchrename::folder_of(p)).or_default().push(i);
        }
        for (dir, idx) in &by_dir {
            let old: Vec<String> = idx.iter().map(|&i| self.files[i].1.name.clone()).collect();
            let nn: Vec<String> = idx.iter().map(|&i| new[i].clone()).collect();
            let taken = &self.taken[dir];
            for (k, p) in batchrename::problems(&old, &nn, |n| taken.contains(n)).into_iter().enumerate() {
                problems[idx[k]] = p;
            }
        }
        let mut changes = 0;
        for (i, (_, s)) in self.files.iter().enumerate() {
            let problem = problems[i].as_deref().map(|p| format!("<span foreground=\"#d03030\">⚠ {}</span>", glib::markup_escape_text(p))).unwrap_or_default();
            if new[i] != s.name {
                changes += 1;
            }
            self.store.insert_with_values(None, &[(COL_OLD, &s.name), (COL_NEW, &new[i]), (COL_PROBLEM, &problem)]);
        }
        let bad = problems.iter().filter(|p| p.is_some()).count();
        if bad > 0 {
            self.status.set_markup(&format!("<b>{bad} of the new names cannot be used.</b>"));
            self.set_ready(None);
        } else if changes == 0 {
            self.status.set_text("No name changes yet.");
            self.set_ready(None);
        } else {
            self.status.set_text(&if changes == 1 { "1 name changes.".to_string() } else { format!("{changes} names change.") });
            self.set_ready(Some(self.files.iter().zip(new).map(|((p, _), n)| (p.clone(), n)).collect()));
        }
    }

    fn set_ready(&self, renames: Option<Vec<(PathBuf, String)>>) {
        self.dialog.set_response_sensitive(gtk::ResponseType::Ok, renames.is_some());
        *self.ready.borrow_mut() = renames;
    }
}

/// Renames on a worker thread; remembers it for Ctrl+Z and selects the new names.
fn run(w: &Window, renames: Vec<(PathBuf, String)>) {
    let weak = Rc::downgrade(&w.me());
    let done = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let d2 = done.clone();
    let n = renames.len();
    w.app.jobs.start(
        w.win.upcast_ref(),
        format!("Renaming {n} items"),
        move |_| {
            let pairs = batchrename::apply(&renames)?;
            *d2.lock().unwrap() = pairs.clone();
            Ok(pairs.into_iter().map(|(_, to)| to).collect())
        },
        move |outcome| {
            let Some(w) = weak.upgrade() else { return };
            let pairs = std::mem::take(&mut *done.lock().unwrap());
            if !pairs.is_empty() {
                w.app.remember(Undoable { what: format!("Rename {} items", pairs.len()), step: Step::Moved(pairs) });
            }
            if let Outcome::Done(paths) = outcome {
                super::fileops::select_paths(&w, paths);
            }
            for p in w.panes() {
                if matches!(p.location(), failbrauwser::location::Location::Search(_)) {
                    p.reload();
                }
            }
        },
    );
}

/// The preview rows as "old -> new" (tests).
pub fn preview_rows() -> Vec<String> {
    let Some(d) = open_dialog() else { return Vec::new() };
    let Some(tree) = find_tree(d.upcast_ref()) else { return Vec::new() };
    let Some(model) = tree.model() else { return Vec::new() };
    let mut out = Vec::new();
    for iter in super::util::child_iters(&model, None) {
        let old: String = model.value(&iter, COL_OLD as i32).get().unwrap_or_default();
        let new: String = model.value(&iter, COL_NEW as i32).get().unwrap_or_default();
        out.push(format!("{old} -> {new}"));
    }
    out
}

/// The open batch rename dialog (tests).
#[allow(deprecated)]
pub fn open_dialog() -> Option<gtk::Dialog> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Dialog>().ok())
        .find(|d| d.is_visible() && d.title().is_some_and(|t| t.starts_with("Rename ") && t.ends_with(" Items")))
}

fn find_tree(w: &gtk::Widget) -> Option<gtk::TreeView> {
    if let Some(t) = w.downcast_ref::<gtk::TreeView>() {
        return Some(t.clone());
    }
    w.downcast_ref::<gtk::Container>()?.children().iter().find_map(find_tree)
}
