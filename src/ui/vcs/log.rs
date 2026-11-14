//! TortoiseGit's "Log Messages" dialog: the revision graph, the kinds of change of each
//! commit (Actions), its branch and tag labels before the message, author and date; below,
//! the whole message of the commit selected and the files it changed (double-click: their
//! diff). A filter, From/To dates, "All Branches", "Show Whole Project", and a right-click
//! menu to switch, branch, tag, reset, revert or cherry-pick at a commit.

use super::super::window::Window;
use super::dialog;
use super::filelist::{self, FileList};
use super::progress;
use super::Wc;
use failbrauwser::vcs::caps::Cap;
use failbrauwser::vcs::git::{self, Commit, GraphRow, RefLabel, Refs, Repo, Status};
use failbrauwser::vcs::ops::{self, Op};
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

/// Commits read at most.
const MAX: usize = 5000;
const ROW_H: i32 = 22;
const LANE_W: i32 = 14;
/// Colors of the graph's lines, column by column.
const LANES: [(f64, f64, f64); 6] = [(0.85, 0.12, 0.12), (0.10, 0.55, 0.15), (0.12, 0.30, 0.85), (0.60, 0.20, 0.70), (0.85, 0.50, 0.00), (0.10, 0.60, 0.65)];

const C_GRAPH: u32 = 0;
const C_ACTIONS: u32 = 1;
const C_MESSAGE: u32 = 2;
const C_AUTHOR: u32 = 3;
const C_DATE: u32 = 4;
const C_INDEX: u32 = 5;

struct Log {
    wc: Wc,
    /// The Git repository, for what only Git has: All Branches, reset, revert, cherry-pick.
    repo: Option<Repo>,
    paths: Vec<String>,
    store: gtk::ListStore,
    view: gtk::TreeView,
    message: gtk::TextView,
    files: Rc<FileList>,
    status: gtk::Label,
    all: gtk::CheckButton,
    whole: gtk::CheckButton,
    filter: gtk::SearchEntry,
    from: gtk::Entry,
    to: gtk::Entry,
    commits: RefCell<Vec<Commit>>,
    refs: RefCell<Refs>,
    graph: RefCell<Vec<GraphRow>>,
    actions: RefCell<HashMap<String, Vec<char>>>,
    shown_files: RefCell<usize>,
}

thread_local! {
    static LOGS: RefCell<Vec<std::rc::Weak<Log>>> = const { RefCell::new(Vec::new()) };
}

pub fn show(w: &Window, wc: &Wc, path: &Path, paths: Vec<String>) {
    let d = dialog::new(w, &dialog::title(path, "Log Messages"), "log");
    d.set_default_size(980, 720);
    let area = d.content_area();
    // Top: the branch, From/To, the filter.
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let head = gtk::Label::new(None);
    head.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&wc.branch_label())));
    top.pack_start(&head, false, false, 0);
    top.pack_start(&dialog::label("From:"), false, false, 6);
    let from = dialog::entry("from", "");
    from.set_width_chars(11);
    from.set_placeholder_text(Some("YYYY-MM-DD"));
    top.pack_start(&from, false, false, 0);
    top.pack_start(&dialog::label("To:"), false, false, 0);
    let to = dialog::entry("to", "");
    to.set_width_chars(11);
    to.set_placeholder_text(Some("YYYY-MM-DD"));
    top.pack_start(&to, false, false, 0);
    let filter = gtk::SearchEntry::new();
    filter.set_widget_name("filter");
    filter.set_placeholder_text(Some("Messages, Authors, Revisions"));
    top.pack_end(&filter, true, true, 0);
    area.pack_start(&top, false, false, 0);

    let store = gtk::ListStore::new(&[Pixbuf::static_type(), Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::U32]);
    let view = gtk::TreeView::with_model(&store);
    view.set_widget_name("commits");
    view.set_enable_search(false);
    for (title, col, kind) in [("Graph", C_GRAPH, 0), ("Actions", C_ACTIONS, 0), ("Message", C_MESSAGE, 1), ("Author", C_AUTHOR, 2), ("Date", C_DATE, 2)] {
        let c = gtk::TreeViewColumn::new();
        c.set_title(title);
        c.set_resizable(true);
        if kind == 0 {
            let r = gtk::CellRendererPixbuf::new();
            r.set_xalign(0.0);
            TreeViewColumnExt::pack_start(&c, &r, false);
            TreeViewColumnExt::add_attribute(&c, &r, "pixbuf", col as i32);
        } else {
            let r = gtk::CellRendererText::new();
            r.set_ellipsize(gtk::pango::EllipsizeMode::End);
            TreeViewColumnExt::pack_start(&c, &r, true);
            TreeViewColumnExt::add_attribute(&c, &r, if kind == 1 { "markup" } else { "text" }, col as i32);
            match title {
                "Message" => c.set_expand(true),
                "Author" => c.set_min_width(130),
                _ => c.set_min_width(150),
            }
        }
        view.append_column(&c);
    }
    let list_sw = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    list_sw.set_shadow_type(gtk::ShadowType::In);
    list_sw.add(&view);
    let (msg_sw, message) = dialog::text_view("log-message", false);
    let files = FileList::new("log-files", false);
    let lower = gtk::Paned::new(gtk::Orientation::Vertical);
    lower.pack1(&msg_sw, false, true);
    lower.pack2(&files.scroll, true, true);
    lower.set_position(140);
    let paned = gtk::Paned::new(gtk::Orientation::Vertical);
    paned.pack1(&list_sw, true, true);
    paned.pack2(&lower, true, true);
    paned.set_position(320);
    paned.set_vexpand(true);
    area.pack_start(&paned, true, true, 0);
    let status = dialog::label("");
    status.set_widget_name("log-status");
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    area.pack_start(&status, false, false, 0);
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let all = dialog::check("All Branches", "all-branches", false);
    let whole = dialog::check("Show Whole Project", "whole-project", paths.iter().all(String::is_empty));
    // Only Git keeps branches a log of the working copy would leave out.
    if wc.as_git().is_some() {
        bottom.pack_start(&all, false, false, 0);
    }
    bottom.pack_start(&whole, false, false, 0);
    let ok = gtk::Button::with_label("OK");
    bottom.pack_end(&ok, false, false, 0);
    area.pack_start(&bottom, false, false, 0);
    let dd = d.clone();
    ok.connect_clicked(move |_| unsafe { dd.destroy() });

    let log = Rc::new(Log {
        wc: wc.clone(),
        repo: wc.as_git().cloned(),
        paths,
        store,
        view: view.clone(),
        message,
        files: files.clone(),
        status,
        all: all.clone(),
        whole: whole.clone(),
        filter: filter.clone(),
        from: from.clone(),
        to: to.clone(),
        commits: RefCell::new(Vec::new()),
        refs: RefCell::new(Refs::default()),
        graph: RefCell::new(Vec::new()),
        actions: RefCell::new(HashMap::new()),
        shown_files: RefCell::new(0),
    });
    dialog::keep_alive(&d, log.clone());
    LOGS.with(|l| {
        let mut l = l.borrow_mut();
        l.retain(|x| x.strong_count() > 0);
        l.push(Rc::downgrade(&log));
    });
    for c in [&all, &whole] {
        let lw = Rc::downgrade(&log);
        c.connect_toggled(move |_| {
            if let Some(l) = lw.upgrade() {
                l.load();
            }
        });
    }
    let lw = Rc::downgrade(&log);
    filter.connect_search_changed(move |_| {
        if let Some(l) = lw.upgrade() {
            l.fill();
        }
    });
    for e in [&from, &to] {
        let lw = Rc::downgrade(&log);
        e.connect_changed(move |_| {
            if let Some(l) = lw.upgrade() {
                l.fill();
            }
        });
    }
    let lw = Rc::downgrade(&log);
    view.selection().connect_changed(move |_| {
        if let Some(l) = lw.upgrade() {
            l.selected_changed();
        }
    });
    let (lw, ww) = (Rc::downgrade(&log), Rc::downgrade(&w.me()));
    files.connect_activated(move |c| {
        let (Some(l), Some(w)) = (lw.upgrade(), ww.upgrade()) else { return };
        if let Some(commit) = l.selected().first() {
            let parent = commit.parents.first().cloned().or_else(|| l.wc.parent(&commit.hash)).unwrap_or_default();
            let old = c.old_path.clone().unwrap_or_else(|| c.path.clone());
            super::diff::revisions(&w, &l.wc, &old, &c.path, &parent, &commit.hash);
        }
    });
    let (lw, ww) = (Rc::downgrade(&log), Rc::downgrade(&w.me()));
    view.connect_button_press_event(move |view, ev| {
        if ev.button() != 3 {
            return glib::Propagation::Proceed;
        }
        let (Some(l), Some(w)) = (lw.upgrade(), ww.upgrade()) else { return glib::Propagation::Proceed };
        let (x, y) = ev.position();
        if let Some((Some(p), ..)) = view.path_at_pos(x as i32, y as i32) {
            view.selection().unselect_all();
            view.selection().select_path(&p);
        }
        if let Some(m) = l.menu(&w) {
            m.set_attach_widget(Some(view));
            m.show_all();
            m.popup_at_pointer(Some(ev));
        }
        glib::Propagation::Stop
    });
    d.show_all();
    log.load();
}

/// Pango markup of a commit's labels and subject: TortoiseGit's colored boxes for the
/// branch checked out (bold), other branches, remote branches, tags and the stash.
pub fn message_markup(labels: &[RefLabel], head: Option<&str>, subject: &str) -> String {
    let mut out = String::new();
    for l in labels {
        let (text, bg, bold) = match l {
            RefLabel::Branch(b) => (b.as_str(), if Some(b.as_str()) == head { "#ff5a5a" } else { "#ffa94d" }, Some(b.as_str()) == head),
            RefLabel::Remote(b) => (b.as_str(), "#a6e3a1", false),
            RefLabel::Tag(t) => (t.as_str(), "#fff36b", false),
            RefLabel::Stash => ("stash", "#c8c8c8", false),
        };
        let weight = if bold { " weight=\"bold\"" } else { "" };
        out.push_str(&format!("<span background=\"{bg}\" foreground=\"#000000\"{weight}> {} </span> ", glib::markup_escape_text(text)));
    }
    out.push_str(&glib::markup_escape_text(subject));
    out
}

/// The graph cell of one row: lines from above and below and the commit's dot.
fn graph_image(row: &GraphRow, max_width: usize) -> Option<Pixbuf> {
    let width = (max_width.max(1) as i32) * LANE_W;
    let surface = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, width, ROW_H).ok()?;
    {
        let cr = gtk::cairo::Context::new(&surface).ok()?;
        let x = |lane: usize| lane as f64 * LANE_W as f64 + LANE_W as f64 / 2.0;
        let mid = ROW_H as f64 / 2.0;
        cr.set_line_width(2.0);
        let color = |cr: &gtk::cairo::Context, lane: usize| {
            let (r, g, b) = LANES[lane % LANES.len()];
            cr.set_source_rgb(r, g, b);
        };
        for &(from, to) in &row.top {
            color(&cr, from);
            cr.move_to(x(from), 0.0);
            if from == to {
                cr.line_to(x(to), mid);
            } else {
                cr.curve_to(x(from), mid * 0.6, x(to), mid * 0.5, x(to), mid);
            }
            let _ = cr.stroke();
        }
        for &(from, to) in &row.bottom {
            color(&cr, to);
            cr.move_to(x(from), mid);
            if from == to {
                cr.line_to(x(to), ROW_H as f64);
            } else {
                cr.curve_to(x(from), mid + mid * 0.5, x(to), mid + mid * 0.4, x(to), ROW_H as f64);
            }
            let _ = cr.stroke();
        }
        color(&cr, row.lane);
        cr.arc(x(row.lane), mid, 4.5, 0.0, std::f64::consts::TAU);
        let _ = cr.fill_preserve();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    }
    gdk::pixbuf_get_from_surface(&surface, 0, 0, width, ROW_H)
}

/// The Actions cell: an icon per kind of change (added, modified, deleted, renamed).
fn actions_image(kinds: &[char]) -> Option<Pixbuf> {
    let size = 14;
    let pb = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, 4 * (size + 2), size)?;
    pb.fill(0);
    for (i, k) in kinds.iter().enumerate() {
        let st = match k {
            'A' => Status::Added,
            'D' => Status::Deleted,
            'R' => Status::Renamed,
            _ => Status::Modified,
        };
        if let Some(ov) = super::icons::overlay(st, size) {
            let x = i as i32 * (size + 2);
            ov.composite(&pb, x, 0, size, size, x as f64, 0.0, 1.0, 1.0, gtk::gdk_pixbuf::InterpType::Bilinear, 255);
        }
    }
    Some(pb)
}

/// "YYYY-MM-DD" as the start of that day (local time).
fn parse_day(s: &str) -> Option<i64> {
    let mut it = s.trim().split('-').map(|x| x.parse::<i32>().ok());
    let (y, m, d) = (it.next()??, it.next()??, it.next()??);
    glib::DateTime::from_local(y, m, d, 0, 0, 0.0).ok().map(|t| t.to_unix())
}

impl Log {
    fn load(self: &Rc<Self>) {
        let (wc, all) = (self.wc.clone(), self.all.is_active());
        let paths = if self.whole.is_active() { Vec::new() } else { self.paths.clone() };
        let me = Rc::downgrade(self);
        self.status.set_text("Loading…");
        super::background(
            move || {
                let (commits, actions) = match wc.as_git() {
                    Some(repo) => (repo.log(all, &paths, MAX).unwrap_or_default(), repo.log_actions(all, &paths, MAX)),
                    None => (wc.log(&paths, MAX).unwrap_or_default(), wc.log_actions(&paths, MAX)),
                };
                let refs = wc.refs();
                // A history of one line draws no graph: the tool records no parents of its own.
                let g = if wc.tool().has(Cap::Graph) { git::graph(&commits.iter().map(|c| (c.hash.clone(), c.parents.clone())).collect::<Vec<_>>()) } else { Vec::new() };
                (commits, refs, actions, g)
            },
            move |(commits, refs, actions, g)| {
                let Some(l) = me.upgrade() else { return };
                *l.commits.borrow_mut() = commits;
                *l.refs.borrow_mut() = refs;
                *l.actions.borrow_mut() = actions;
                *l.graph.borrow_mut() = g;
                l.fill();
                if let Some(first) = l.store.iter_first() {
                    l.view.selection().select_iter(&first);
                }
            },
        );
    }

    /// Shows the commits the filter and dates let through.
    fn fill(&self) {
        self.store.clear();
        let q = self.filter.text().to_lowercase();
        let (from, to) = (parse_day(&self.from.text()), parse_day(&self.to.text()).map(|t| t + 86_400));
        let commits = self.commits.borrow();
        let graph = self.graph.borrow();
        let refs = self.refs.borrow();
        let actions = self.actions.borrow();
        let head = match &self.repo {
            Some(r) => r.branch(),
            None => Some(self.wc.branch_label()),
        };
        let filtered = !q.is_empty() || from.is_some() || to.is_some();
        let max_width = graph.iter().map(|g| g.width).max().unwrap_or(1).min(16);
        let mut shown = 0;
        for (i, c) in commits.iter().enumerate() {
            if !q.is_empty() && !(c.message.to_lowercase().contains(&q) || c.author.to_lowercase().contains(&q) || c.hash.starts_with(&q)) {
                continue;
            }
            if from.is_some_and(|f| c.time < f) || to.is_some_and(|t| c.time >= t) {
                continue;
            }
            // A filtered list has no graph (its lines would not connect).
            let g = if filtered { None } else { graph.get(i).and_then(|g| graph_image(g, max_width)) };
            let labels = refs.by_commit.get(&c.hash).cloned().unwrap_or_default();
            let it = self.store.append();
            self.store.set(
                &it,
                &[
                    (C_GRAPH, &g),
                    (C_ACTIONS, &actions.get(&c.hash).and_then(|k| actions_image(k))),
                    (C_MESSAGE, &message_markup(&labels, head.as_deref(), c.subject())),
                    (C_AUTHOR, &c.author),
                    (C_DATE, &dialog::when(c.time)),
                    (C_INDEX, &(i as u32)),
                ],
            );
            shown += 1;
        }
        drop((commits, graph, refs, actions));
        self.update_status(shown);
    }

    fn selected(&self) -> Vec<Commit> {
        let (rows, model) = self.view.selection().selected_rows();
        let commits = self.commits.borrow();
        rows.iter()
            .filter_map(|p| model.iter(p))
            .filter_map(|it| model.value(&it, C_INDEX as i32).get::<u32>().ok())
            .filter_map(|i| commits.get(i as usize).cloned())
            .collect()
    }

    fn selected_changed(self: &Rc<Self>) {
        let sel = self.selected();
        let buf = self.message.buffer().expect("buffer");
        let Some(c) = sel.first().cloned() else {
            buf.set_text("");
            self.files.set(Vec::new(), |_| false);
            return;
        };
        let author = if c.email.is_empty() { c.author.clone() } else { format!("{} <{}>", c.author, c.email) };
        buf.set_text(&format!(
            "{}: {}\nAuthor: {}\nDate: {}\nParents: {}\n\n{}",
            if self.repo.is_some() { "SHA-1" } else { "Revision" },
            c.hash,
            author,
            dialog::when(c.time),
            c.parents.iter().map(|p| &p[..p.len().min(8)]).collect::<Vec<_>>().join(", "),
            c.message
        ));
        let (wc, me) = (self.wc.clone(), Rc::downgrade(self));
        let hash = c.hash.clone();
        super::background(move || wc.revision_changes(&hash).unwrap_or_default(), move |changes| {
            let Some(l) = me.upgrade() else { return };
            // Still the commit selected?
            if l.selected().first().is_some_and(|s| s.hash == c.hash) {
                *l.shown_files.borrow_mut() = changes.len();
                l.files.set(changes, |_| false);
                l.update_status(l.store.iter_n_children(None) as usize);
            }
        });
    }

    fn update_status(&self, shown: usize) {
        let commits = self.commits.borrow();
        let mut text = format!("Showing {shown} revision(s)");
        if let (Some(first), Some(last)) = (commits.last(), commits.first()) {
            text.push_str(&format!(", from revision {} to revision {}", first.short(), last.short()));
        }
        let sel = self.view.selection().count_selected_rows();
        if sel > 0 {
            let files = self.files.all();
            let count = |st: Status| files.iter().filter(|c| c.status == st).count();
            text.push_str(&format!(
                " - {sel} revision(s) selected, showing {} changed paths, {} added, {} modified, {} deleted",
                files.len(),
                count(Status::Added) + count(Status::Renamed),
                count(Status::Modified),
                count(Status::Deleted)
            ));
        }
        self.status.set_text(&text);
    }

    /// The right-click menu of a revision of a tool other than Git: the working copy brought to
    /// it, and its name copied.
    fn tool_menu(self: &Rc<Self>, w: &Window, c: &Commit) -> gtk::Menu {
        let m = gtk::Menu::new();
        let tool = self.wc.tool();
        let runs = ops::runs(tool, &Op::UpdateTo { rev: c.hash.clone() });
        if !runs.is_empty() {
            let it = filelist::menu_item("Update to this revision...", "switch");
            let (wc, short) = (self.wc.clone(), c.short().to_string());
            let ww = Rc::downgrade(&w.me());
            it.connect_activate(move |_| {
                let Some(w) = ww.upgrade() else { return };
                let (wc, runs) = (wc.clone(), runs.clone());
                let w2 = Rc::downgrade(&w.me());
                dialog::confirm(&w.win, &format!("Bring the working copy to revision {short}?"), "", "Update", move || {
                    if let Some(w) = w2.upgrade() {
                        progress::run(&w, wc.root(), wc.root(), progress::tool_cmds(wc.tool(), runs.clone()), Vec::new(), |w, _| super::overlay::refresh_all(w));
                    }
                });
            });
            m.append(&it);
            m.append(&gtk::SeparatorMenuItem::new());
        }
        let copy = filelist::menu_item("Copy revision to clipboard", "log");
        let h = c.hash.clone();
        copy.connect_activate(move |_| gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&h));
        m.append(&copy);
        m
    }

    /// The right-click menu of the commit selected.
    fn menu(self: &Rc<Self>, w: &Window) -> Option<gtk::Menu> {
        let c = self.selected().first().cloned()?;
        let Some(repo) = self.repo.clone() else { return Some(self.tool_menu(w, &c)) };
        let m = gtk::Menu::new();
        let short = c.short().to_string();
        let branch = repo.branch_label();
        let entries: Vec<(String, &str, Rc<dyn Fn(&Window)>)> = vec![
            ("Switch/Checkout to this...".into(), "switch", { let (r, h) = (repo.clone(), c.hash.clone()); Rc::new(move |w: &Window| super::dialogs::switch_to(w, &r, &h)) }),
            ("Create Branch at this version...".into(), "branch", { let (r, h) = (repo.clone(), c.hash.clone()); Rc::new(move |w: &Window| super::dialogs::branch_at(w, &r, &h)) }),
            ("Create Tag at this version...".into(), "tag", { let (r, h) = (repo.clone(), c.hash.clone()); Rc::new(move |w: &Window| super::dialogs::tag_at(w, &r, &h)) }),
            (format!("Reset \"{branch}\" to this..."), "revert", { let (r, h) = (repo.clone(), c.hash.clone()); Rc::new(move |w: &Window| super::dialogs::reset_to(w, &r, &h)) }),
            ("Revert change by this commit".into(), "revert", {
                let (r, h, s) = (repo.clone(), c.hash.clone(), short.clone());
                Rc::new(move |w: &Window| {
                    let (r, h) = (r.clone(), h.clone());
                    let ww = Rc::downgrade(&w.me());
                    dialog::confirm(&w.win, &format!("Do you really want to revert the changes of commit {s}?"), "A new commit takes them back.", "Revert", move || {
                        if let Some(w) = ww.upgrade() {
                            progress::run(&w, &r.root, &r.root, vec![progress::cmd(&["revert", "--no-edit", &h])], Vec::new(), |_, _| {});
                        }
                    });
                })
            }),
            ("Cherry Pick this commit...".into(), "commit", {
                let (r, h, s) = (repo.clone(), c.hash.clone(), short.clone());
                Rc::new(move |w: &Window| {
                    let (r, h) = (r.clone(), h.clone());
                    let ww = Rc::downgrade(&w.me());
                    dialog::confirm(&w.win, &format!("Cherry pick commit {s} onto the current branch?"), "", "Cherry Pick", move || {
                        if let Some(w) = ww.upgrade() {
                            progress::run(&w, &r.root, &r.root, vec![progress::cmd(&["cherry-pick", &h])], Vec::new(), |_, _| {});
                        }
                    });
                })
            }),
        ];
        for (label, icon, f) in entries {
            let it = filelist::menu_item(&label, icon);
            let ww = Rc::downgrade(&w.me());
            it.connect_activate(move |_| {
                if let Some(w) = ww.upgrade() {
                    f(&w);
                }
            });
            m.append(&it);
        }
        m.append(&gtk::SeparatorMenuItem::new());
        let copy = filelist::menu_item("Copy SHA-1 to clipboard", "log");
        let h = c.hash.clone();
        copy.connect_activate(move |_| gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&h));
        m.append(&copy);
        Some(m)
    }
}

/// The open log whose window title contains `title` (tests): its rows as "subject" with
/// their labels in brackets, and selecting one by subject.
pub fn rows(title: &str) -> Option<Vec<String>> {
    let l = find(title)?;
    let commits = l.commits.borrow();
    let refs = l.refs.borrow();
    let mut out = Vec::new();
    l.store.foreach(|m, _, it| {
        let i: u32 = m.value(it, C_INDEX as i32).get().unwrap_or(0);
        if let Some(c) = commits.get(i as usize) {
            let labels: Vec<String> = refs
                .by_commit
                .get(&c.hash)
                .map(|ls| ls.iter().map(|l| match l {
                    RefLabel::Branch(b) | RefLabel::Remote(b) | RefLabel::Tag(b) => b.clone(),
                    RefLabel::Stash => "stash".into(),
                }).collect())
                .unwrap_or_default();
            out.push(if labels.is_empty() { c.subject().to_string() } else { format!("[{}] {}", labels.join(","), c.subject()) });
        }
        false
    });
    Some(out)
}

pub fn select(title: &str, subject: &str) -> bool {
    let Some(l) = find(title) else { return false };
    let commits = l.commits.borrow().clone();
    let mut found = None;
    l.store.foreach(|m, p, it| {
        let i: u32 = m.value(it, C_INDEX as i32).get().unwrap_or(0);
        if commits.get(i as usize).is_some_and(|c| c.subject() == subject) {
            found = Some(p.clone());
            return true;
        }
        false
    });
    match found {
        Some(p) => {
            l.view.selection().unselect_all();
            l.view.selection().select_path(&p);
            true
        }
        None => false,
    }
}

/// Picks an item of the selected commit's right-click menu by its text (tests).
pub fn menu_item(w: &Window, title: &str, label: &str) -> Result<(), String> {
    let l = find(title).ok_or("no log")?;
    let m = l.menu(w).ok_or("nothing selected")?;
    let item = m.children().into_iter().filter_map(|c| c.downcast::<gtk::MenuItem>().ok()).find(|mi| {
        mi.child().and_then(|c| c.downcast::<gtk::Box>().ok()).is_some_and(|b| b.children().iter().any(|c| c.downcast_ref::<gtk::Label>().is_some_and(|l| l.text() == label)))
    });
    item.ok_or_else(|| format!("no item {label:?}"))?.activate();
    Ok(())
}

fn find(title: &str) -> Option<Rc<Log>> {
    LOGS.with(|l| {
        l.borrow().iter().filter_map(|x| x.upgrade()).find(|l| {
            l.view.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok()).is_some_and(|t| t.is_visible() && t.title().is_some_and(|x| x.contains(title)))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_before_the_subject() {
        let m = message_markup(&[RefLabel::Branch("main".into()), RefLabel::Tag("v1".into()), RefLabel::Remote("origin/main".into())], Some("main"), "Fix <it>");
        assert!(m.starts_with("<span background=\"#ff5a5a\" foreground=\"#000000\" weight=\"bold\"> main </span> "));
        assert!(m.contains("#fff36b") && m.contains("origin/main"));
        assert!(m.ends_with("Fix &lt;it&gt;"));
        assert_eq!(message_markup(&[], None, "plain"), "plain");
    }
}
