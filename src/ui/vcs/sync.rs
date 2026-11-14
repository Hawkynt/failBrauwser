//! TortoiseGit's "Git Synchronization": the local branch, the remote branch and the remote,
//! buttons to pull, fetch, push and commit, the commits going out and coming in, and the
//! output of what ran.

use super::super::window::Window;
use super::dialog;
use super::progress::{self, Event};
use super::Target;
use failbrauwser::vcs::git::{Commit, Repo};
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct Sync {
    repo: Repo,
    local: gtk::ComboBoxText,
    remote_branch: gtk::ComboBoxText,
    remote: gtk::ComboBoxText,
    out_store: gtk::ListStore,
    in_store: gtk::ListStore,
    output: gtk::TextView,
    notebook: gtk::Notebook,
    buttons: Vec<gtk::Widget>,
    running: Cell<bool>,
}

fn commit_list(name: &str) -> (gtk::ScrolledWindow, gtk::ListStore) {
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let view = gtk::TreeView::with_model(&store);
    view.set_widget_name(name);
    for (i, t) in ["Commit", "Message", "Author", "Date"].iter().enumerate() {
        let c = gtk::TreeViewColumn::new();
        c.set_title(t);
        c.set_resizable(true);
        let r = gtk::CellRendererText::new();
        r.set_ellipsize(gtk::pango::EllipsizeMode::End);
        if i == 0 {
            r.set_property("family", "monospace");
        }
        TreeViewColumnExt::pack_start(&c, &r, true);
        TreeViewColumnExt::add_attribute(&c, &r, "text", i as i32);
        if i == 1 {
            c.set_expand(true);
        }
        view.append_column(&c);
    }
    let sw = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.add(&view);
    (sw, store)
}

fn fill(store: &gtk::ListStore, commits: &[Commit]) {
    store.clear();
    for c in commits {
        let it = store.append();
        store.set(&it, &[(0, &c.short()), (1, &c.subject()), (2, &c.author), (3, &dialog::when(c.time))]);
    }
}

pub fn show(w: &Window, t: Target, repo: Repo) {
    let d = dialog::new(w, &dialog::title(&t.title_path(), "Git Synchronization"), "sync");
    d.set_default_size(820, 560);
    let area = d.content_area();
    let refs = repo.refs();
    let branch = repo.branch().unwrap_or_default();
    let up = repo.upstream(&branch);
    let remotes = repo.remotes();
    let up_remote = up.as_ref().and_then(|u| remotes.iter().find(|r| u.starts_with(&format!("{r}/"))).cloned());
    let up_branch = match (&up, &up_remote) {
        (Some(u), Some(r)) => u[r.len() + 1..].to_string(),
        _ => branch.clone(),
    };
    let g = dialog::grid();
    let local = dialog::combo("local", &refs.branches, &branch);
    let remote_branch = dialog::combo("remote-branch", &[], &up_branch);
    let remote = dialog::combo("remote", &remotes, up_remote.as_deref().or(remotes.first().map(String::as_str)).unwrap_or(""));
    dialog::grid_row(&g, 0, "Local Branch:", &local);
    dialog::grid_row(&g, 1, "Remote Branch:", &remote_branch);
    dialog::grid_row(&g, 2, "Remote URL:", &remote);
    let url = dialog::label(&repo.remote_url(&dialog::combo_text(&remote)));
    url.set_widget_name("remote-url");
    url.set_selectable(true);
    g.attach(&url, 1, 3, 1, 1);
    area.pack_start(&g, false, false, 0);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let left = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let mut buttons: Vec<gtk::Widget> = Vec::new();
    let mk = |label: &str, icon: &str| {
        let b = gtk::Button::new();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.pack_start(&super::icons::image(icon, 24), false, false, 0);
        row.pack_start(&gtk::Label::new(Some(label)), false, false, 0);
        b.add(&row);
        b.set_widget_name(&format!("sync-{}", label.to_lowercase()));
        b
    };
    let pull = mk("Pull", "pull");
    let fetch = mk("Fetch", "fetch");
    let push = mk("Push", "push");
    let commit = mk("Commit", "commit");
    for b in [&pull, &fetch, &push, &commit] {
        left.pack_start(b, false, false, 0);
        buttons.push(b.clone().upcast());
    }
    body.pack_start(&left, false, false, 0);
    let notebook = gtk::Notebook::new();
    let (out_sw, out_store) = commit_list("out-commits");
    let (in_sw, in_store) = commit_list("in-commits");
    let (log_sw, output) = dialog::text_view("sync-output", false);
    output.set_wrap_mode(gtk::WrapMode::None);
    notebook.append_page(&out_sw, Some(&gtk::Label::new(Some("Out Commits"))));
    notebook.append_page(&in_sw, Some(&gtk::Label::new(Some("In Commits"))));
    notebook.append_page(&log_sw, Some(&gtk::Label::new(Some("Git Output Log"))));
    body.pack_start(&notebook, true, true, 0);
    area.pack_start(&body, true, true, 0);
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let log_btn = gtk::Button::with_label("Log");
    let close = gtk::Button::with_label("Close");
    bottom.pack_start(&log_btn, false, false, 0);
    bottom.pack_end(&close, false, false, 0);
    area.pack_start(&bottom, false, false, 0);
    let dd = d.clone();
    close.connect_clicked(move |_| unsafe { dd.destroy() });

    let s = Rc::new(Sync { repo: repo.clone(), local, remote_branch, remote: remote.clone(), out_store, in_store, output, notebook, buttons, running: Cell::new(false) });
    dialog::keep_alive(&d, s.clone());
    let (sw, u2) = (Rc::downgrade(&s), url.clone());
    remote.connect_changed(move |c| {
        if let Some(s) = sw.upgrade() {
            u2.set_text(&s.repo.remote_url(&dialog::combo_text(c)));
            s.refresh();
        }
    });
    let ww = Rc::downgrade(&w.me());
    let sw = Rc::downgrade(&s);
    pull.connect_clicked(move |_| {
        if let Some(s) = sw.upgrade() {
            let (r, b) = (s.remote_name(), dialog::combo_text(&s.remote_branch));
            s.run(vec![super::dialogs::pull_cmd(&r, &b, false, false, false, false, false, false)]);
        }
    });
    let sw = Rc::downgrade(&s);
    fetch.connect_clicked(move |_| {
        if let Some(s) = sw.upgrade() {
            let r = s.remote_name();
            s.run(vec![progress::cmd(&["fetch", "--progress", "-v", &r])]);
        }
    });
    let sw = Rc::downgrade(&s);
    push.connect_clicked(move |_| {
        if let Some(s) = sw.upgrade() {
            let (r, l, b) = (s.remote_name(), dialog::combo_text(&s.local), dialog::combo_text(&s.remote_branch));
            let track = s.repo.upstream(&l).is_none();
            s.run(vec![super::dialogs::push_cmd(Some(&r), &l, &b, false, None, false, track)]);
        }
    });
    let (ww2, r2) = (ww.clone(), repo.clone());
    commit.connect_clicked(move |_| {
        if let Some(w) = ww2.upgrade() {
            super::commit::show(&w, &super::wc(&r2), &r2.root, vec![String::new()]);
        }
    });
    let r2 = repo.clone();
    log_btn.connect_clicked(move |_| {
        if let Some(w) = ww.upgrade() {
            super::log::show(&w, &super::wc(&r2), &r2.root, Vec::new());
        }
    });
    let sw = Rc::downgrade(&s);
    d.connect_is_active_notify(move |d| {
        if d.is_active() {
            if let Some(s) = sw.upgrade() {
                s.refresh();
            }
        }
    });
    d.show_all();
    s.refresh();
}

impl Sync {
    fn remote_name(&self) -> String {
        let r = dialog::combo_text(&self.remote);
        if r.is_empty() { "origin".into() } else { r }
    }

    /// The commits going out (here, not there) and coming in (there, not here).
    fn refresh(self: &Rc<Self>) {
        let theirs = format!("{}/{}", self.remote_name(), dialog::combo_text(&self.remote_branch));
        let ours = dialog::combo_text(&self.local);
        let (repo, me) = (self.repo.clone(), Rc::downgrade(self));
        super::background(
            move || {
                let exists = repo.run(&["rev-parse", "--verify", "-q", &format!("refs/remotes/{theirs}")]).is_ok();
                if exists { (repo.log_range(&format!("{theirs}..{ours}")), repo.log_range(&format!("{ours}..{theirs}"))) } else { (repo.log_range(&ours), Vec::new()) }
            },
            move |(out, inc)| {
                if let Some(s) = me.upgrade() {
                    fill(&s.out_store, &out);
                    fill(&s.in_store, &inc);
                }
            },
        );
    }

    fn append(&self, text: &str, color: Option<&str>) {
        let b = self.output.buffer().expect("buffer");
        let mut end = b.end_iter();
        match color {
            Some(c) => b.insert_markup(&mut end, &format!("<b><span foreground=\"{c}\">{}</span></b>", glib::markup_escape_text(text))),
            None => b.insert(&mut end, text),
        }
        if let Some(mark) = b.create_mark(None, &b.end_iter(), false) {
            self.output.scroll_mark_onscreen(&mark);
            b.delete_mark(&mark);
        }
    }

    /// Runs the commands, their output in "Git Output Log".
    fn run(self: &Rc<Self>, cmds: Vec<progress::Cmd>) {
        if self.running.replace(true) {
            return;
        }
        self.buttons.iter().for_each(|b| b.set_sensitive(false));
        self.notebook.set_current_page(Some(2));
        let rx = progress::execute(&self.repo.root, cmds.clone(), Arc::new(Mutex::new(None)));
        let me = self.clone();
        let started = Instant::now();
        glib::spawn_future_local(async move {
            while let Ok(ev) = rx.recv().await {
                match ev {
                    Event::Start(i) => me.append(&format!("{}\n", cmds[i].display()), Some("#1d4fbf")),
                    Event::Out(s) => me.append(&s, None),
                    Event::Progress(_) => {}
                    Event::Done(code) => {
                        let now = glib::DateTime::now_local().ok().and_then(|d| d.format("%Y-%m-%d %H:%M:%S").ok()).map(|s| s.to_string()).unwrap_or_default();
                        let text = format!("{} ({} ms @ {now})\n\n", progress::outcome("git", code, false), started.elapsed().as_millis());
                        me.append(&text, Some(if code == Some(0) { "#1f8f2f" } else { "#c62020" }));
                    }
                }
            }
            me.running.set(false);
            me.buttons.iter().for_each(|b| b.set_sensitive(true));
            me.refresh();
        });
    }
}

/// Whether the sync window titled with `title` is running something (tests).
pub fn busy(title: &str) -> bool {
    dialog::find_window(title).is_some_and(|w| {
        fn find(w: &gtk::Widget) -> Option<gtk::Widget> {
            if w.widget_name() == "sync-pull" {
                return Some(w.clone());
            }
            w.downcast_ref::<gtk::Container>()?.children().iter().find_map(find)
        }
        find(w.upcast_ref()).is_some_and(|b| !b.is_sensitive())
    })
}
