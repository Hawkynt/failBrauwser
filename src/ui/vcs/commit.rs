//! TortoiseGit's Commit dialog: "Commit to:" the branch (or a new one), the message,
//! "Amend Last Commit", the author, and "Changes made (double-click on file for diff):" with
//! a check box per file, the "Check: All None Unversioned …" links, the number of files
//! selected, "Show Unversioned Files" and "Show Whole Project"; Commit, or Commit & Push.

use super::super::window::Window;
use super::dialog;
use super::filelist::{self, FileList};
use super::progress::{self, Cmd};
use super::Wc;
use failbrauwser::vcs::caps::Cap;
use failbrauwser::vcs::git::{Change, Repo, Status};
use failbrauwser::vcs::ops::{self, Op};
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

/// The commands that commit exactly the files ticked: changes of other files staged before
/// are taken out of the index first (as TortoiseGit commits what is ticked), the ticked ones
/// put in, then the commit.
pub fn commit_cmds(has_commits: bool, staged_elsewhere: &[Change], checked: &[Change], message: &str, amend: bool, author: Option<&str>, sign: bool, new_branch: Option<&str>) -> Vec<Cmd> {
    let mut out = Vec::new();
    if let Some(b) = new_branch {
        out.push(progress::cmd(&["checkout", "-b", b]));
    }
    let picked: Vec<&str> = checked.iter().flat_map(|c| std::iter::once(c.path.as_str()).chain(c.old_path.as_deref())).collect();
    let unstage: Vec<&str> = staged_elsewhere
        .iter()
        .filter(|c| c.staged && !picked.contains(&c.path.as_str()))
        .flat_map(|c| std::iter::once(c.path.as_str()).chain(c.old_path.as_deref()))
        .filter(|p| !picked.contains(p))
        .collect();
    if !unstage.is_empty() {
        let mut a: Vec<&str> = if has_commits { vec!["reset", "-q", "--"] } else { vec!["rm", "--cached", "-r", "-q", "--ignore-unmatch", "--"] };
        a.extend(unstage);
        out.push(progress::cmd(&a));
    }
    if !picked.is_empty() {
        let mut a = vec!["add", "-A", "--"];
        a.extend(picked.iter().copied());
        out.push(progress::cmd(&a));
    }
    let mut a: Vec<String> = vec!["commit".into()];
    if amend {
        a.push("--amend".into());
    }
    if checked.is_empty() {
        a.push("--allow-empty".into());
    }
    if let Some(au) = author.filter(|a| !a.trim().is_empty()) {
        a.push(format!("--author={}", au.trim()));
    }
    if sign {
        a.push("-s".into());
    }
    a.extend(["-F".into(), "-".into()]);
    out.push(progress::cmd(&a).input(message));
    out
}

/// TortoiseGit's "Check:" links: which changes each one ticks.
pub fn check_filter(which: &str, c: &Change) -> bool {
    match which {
        "All" => true,
        "None" => false,
        "Unversioned" => c.status == Status::Unversioned,
        "Versioned" => c.status != Status::Unversioned && c.status != Status::Ignored,
        "Added" => matches!(c.status, Status::Added | Status::Renamed),
        "Deleted" => c.status == Status::Deleted,
        "Modified" => matches!(c.status, Status::Modified | Status::Conflicted),
        // Files (not submodules: everything here is a file).
        _ => true,
    }
}

pub fn show(w: &Window, wc: &Wc, path: &Path, paths: Vec<String>) {
    show_with(w, wc, path, paths, false);
}

/// What a tool other than Git runs for a commit of the ticked files: the unversioned ones among
/// them added first, then the commit of exactly those, then a push where asked.
pub fn tool_commit_ops(checked: &[Change], message: &str, amend: bool, push: bool) -> Vec<Op> {
    let mut out = Vec::new();
    let new: Vec<String> = checked.iter().filter(|c| c.status == Status::Unversioned).map(|c| c.path.clone()).collect();
    if !new.is_empty() {
        out.push(Op::Add { paths: new });
    }
    out.push(Op::Commit { paths: checked.iter().map(|c| c.path.clone()).collect(), message: message.to_string(), amend });
    if push {
        out.push(Op::Push);
    }
    out
}

/// The Commit dialog; `push` ticks Commit & Push from the start.
pub fn show_with(w: &Window, wc: &Wc, path: &Path, paths: Vec<String>, push: bool) {
    // Git's own options (a new branch, the author, Signed-off-by) are Git's alone.
    let repo: Option<Repo> = wc.as_git().cloned();
    let tool = wc.tool();
    let d = dialog::new(w, &dialog::title(path, "Commit"), "commit");
    d.set_default_size(760, 640);
    let area = d.content_area();
    // "Commit to: main  [ ] new branch"
    let to = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    to.pack_start(&dialog::label("Commit to:"), false, false, 0);
    let branch = gtk::Label::new(None);
    branch.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&wc.branch_label())));
    to.pack_start(&branch, false, false, 0);
    let new_branch = dialog::check("new branch", "new-branch", false);
    let new_name = dialog::entry("new-branch-name", "");
    new_name.set_sensitive(false);
    if repo.is_some() {
        to.pack_start(&new_branch, false, false, 12);
        to.pack_start(&new_name, false, false, 0);
    }
    let nn = new_name.clone();
    new_branch.connect_toggled(move |c| nn.set_sensitive(c.is_active()));
    area.pack_start(&to, false, false, 0);

    // Message
    let msg_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let (msg_sw, msg) = dialog::text_view("message", true);
    msg_sw.set_size_request(-1, 130);
    msg_box.pack_start(&msg_sw, true, true, 0);
    let opts = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let amend = dialog::check("Amend Last Commit", "amend", false);
    amend.set_sensitive(repo.as_ref().map_or(tool.has(Cap::Amend), |r| r.has_commits()));
    let set_author = dialog::check("Set author", "set-author", false);
    let author = dialog::entry("author", &repo.as_ref().map(|r| format!("{} <{}>", r.config("user.name", None), r.config("user.email", None))).unwrap_or_default());
    author.set_sensitive(false);
    let sign = dialog::check("Add \"Signed-off-by\"", "sign", false);
    if tool.has(Cap::Amend) {
        opts.pack_start(&amend, false, false, 0);
    }
    if repo.is_some() {
        opts.pack_start(&set_author, false, false, 0);
        opts.pack_start(&author, true, true, 0);
        opts.pack_start(&sign, false, false, 0);
    }
    msg_box.pack_start(&opts, false, false, 0);
    let au = author.clone();
    set_author.connect_toggled(move |c| au.set_sensitive(c.is_active()));
    let msg_frame = dialog::frame("Message:", &msg_box);
    area.pack_start(&msg_frame, false, false, 0);
    let (r2, m2) = (wc.clone(), msg.clone());
    amend.connect_toggled(move |c| {
        // Amending starts from the message of the last commit.
        let b = m2.buffer().expect("buffer");
        if c.is_active() && dialog::buffer_text(&m2).trim().is_empty() {
            if let Some(last) = r2.log(&[], 1).ok().and_then(|l| l.into_iter().next()) {
                b.set_text(last.message.trim_end());
            }
        }
    });

    // Changes made
    area.pack_start(&dialog::label("Changes made (double-click on file for diff):"), false, false, 0);
    let list = FileList::new("changes", true);
    area.pack_start(&list.scroll, true, true, 0);
    let checks = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    checks.pack_start(&dialog::label("Check:"), false, false, 4);
    for which in ["All", "None", "Unversioned", "Versioned", "Added", "Deleted", "Modified", "Files"] {
        let l = gtk::LinkButton::with_label("", which);
        l.set_widget_name(&format!("check-{}", which.to_lowercase()));
        let lw = Rc::downgrade(&list);
        l.connect_activate_link(move |_| {
            if let Some(list) = lw.upgrade() {
                list.check_where(|c| check_filter(which, c));
            }
            glib::Propagation::Stop
        });
        checks.pack_start(&l, false, false, 0);
    }
    let count = dialog::label("");
    count.set_widget_name("count");
    checks.pack_end(&count, false, false, 4);
    area.pack_start(&checks, false, false, 0);
    let shows = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let show_unversioned = dialog::check("Show Unversioned Files", "show-unversioned", false);
    let whole = dialog::check("Show Whole Project", "whole-project", paths.iter().any(String::is_empty));
    shows.pack_start(&show_unversioned, false, false, 0);
    shows.pack_start(&whole, false, false, 0);
    area.pack_start(&shows, false, false, 0);

    // Commit (with Commit & Push in its drop-down), Cancel.
    let commit_btn = gtk::Button::with_label("Commit");
    commit_btn.set_widget_name("commit");
    let and_push = Rc::new(Cell::new(push));
    if push {
        commit_btn.set_label("Commit & Push");
    }
    let more = gtk::MenuButton::new();
    more.set_widget_name("commit-more");
    let pop = gtk::Menu::new();
    for (label, p) in [("Commit", false), ("Commit & Push", true)] {
        let mi = gtk::MenuItem::with_label(label);
        let (b, ap) = (commit_btn.clone(), and_push.clone());
        mi.connect_activate(move |_| {
            ap.set(p);
            b.set_label(label);
        });
        pop.append(&mi);
    }
    pop.show_all();
    more.set_popup(Some(&pop));
    let split = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    split.style_context().add_class("linked");
    split.pack_start(&commit_btn, false, false, 0);
    // Commit & Push only where there is something to push to.
    if tool.has(Cap::Push) {
        split.pack_start(&more, false, false, 0);
    }
    let cancel = gtk::Button::with_label("Cancel");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.pack_end(&cancel, false, false, 0);
    buttons.pack_end(&split, false, false, 0);
    area.pack_start(&buttons, false, false, 0);
    let dd = d.clone();
    cancel.connect_clicked(move |_| unsafe { dd.destroy() });

    // What is shown and how many are ticked.
    let update = {
        let (count, list, commit_btn, msg, amend) = (count.clone(), Rc::downgrade(&list), commit_btn.clone(), msg.clone(), amend.clone());
        Rc::new(move || {
            let Some(l) = list.upgrade() else { return };
            let (sel, total) = (l.checked().len(), l.all().len());
            count.set_text(&format!("{sel} files selected, {total} files total"));
            let has_msg = !dialog::buffer_text(&msg).trim().is_empty();
            commit_btn.set_sensitive(has_msg && (sel > 0 || amend.is_active()));
        })
    };
    let u = update.clone();
    list.connect_toggled(move || u());
    let u = update.clone();
    msg.buffer().expect("buffer").connect_changed(move |_| u());
    let u = update.clone();
    amend.connect_toggled(move |_| u());
    let reload = {
        let (repo, list, show_unversioned, whole, paths) = (wc.clone(), Rc::downgrade(&list), show_unversioned.clone(), whole.clone(), paths.clone());
        Rc::new(move || {
            let (r, unv) = (repo.clone(), show_unversioned.is_active());
            let p = if whole.is_active() { Vec::new() } else { paths.clone() };
            let lw = list.clone();
            super::background(move || r.changes(&p, unv, false), move |res| {
                if let (Some(l), Ok(changes)) = (lw.upgrade(), res) {
                    l.set(changes, |c| c.status.is_change());
                }
            });
        })
    };
    for c in [&show_unversioned, &whole] {
        let r = reload.clone();
        c.connect_toggled(move |_| r());
    }
    reload();
    let (ww, r2) = (Rc::downgrade(&w.me()), wc.clone());
    list.connect_activated(move |c| {
        if let Some(w) = ww.upgrade() {
            super::diff::working(&w, &r2, &c.path);
        }
    });
    let (ww, r2) = (Rc::downgrade(&w.me()), wc.clone());
    list.connect_menu(move |sel| {
        let m = gtk::Menu::new();
        let one = sel.first().cloned()?;
        let diff = filelist::menu_item("Compare with base", "diff");
        let (w1, r1, c1) = (ww.clone(), r2.clone(), one.clone());
        diff.connect_activate(move |_| {
            if let Some(w) = w1.upgrade() {
                super::diff::working(&w, &r1, &c1.path);
            }
        });
        m.append(&diff);
        let log = filelist::menu_item("Show log", "log");
        let (w1, r1, c1) = (ww.clone(), r2.clone(), one.clone());
        log.connect_activate(move |_| {
            if let Some(w) = w1.upgrade() {
                super::log::show(&w, &r1, &r1.root().join(&c1.path), vec![c1.path.clone()]);
            }
        });
        m.append(&log);
        let revert = filelist::menu_item("Revert...", "revert");
        let (w1, r1, files) = (ww.clone(), r2.clone(), sel.to_vec());
        revert.connect_activate(move |_| {
            if let Some(w) = w1.upgrade() {
                super::revert_files(&w, &r1, &files);
            }
        });
        m.append(&revert);
        Some(m)
    });
    // Reverting from the list or elsewhere: read it again when the dialog is used again.
    let r = reload.clone();
    d.connect_is_active_notify(move |d| {
        if d.is_active() {
            r();
        }
    });

    let (ww, repo2, dd, wc2) = (Rc::downgrade(&w.me()), repo.clone(), d.clone(), wc.clone());
    let lw = Rc::downgrade(&list);
    let msg2 = msg.clone();
    commit_btn.connect_clicked(move |_| {
        let msg = &msg2;
        let (Some(w), Some(l)) = (ww.upgrade(), lw.upgrade()) else { return };
        let message = dialog::buffer_text(&msg);
        let checked = l.checked();
        let Some(repo2) = repo2.clone() else {
            // Another tool: its own client commits exactly what is ticked.
            let tool = wc2.tool();
            let cmds: Vec<Cmd> = tool_commit_ops(&checked, &message, amend.is_active(), and_push.get()).iter().flat_map(|op| progress::tool_cmds(tool, ops::runs(tool, op))).collect();
            let wc3 = wc2.clone();
            let posts: Vec<progress::Post> = if !and_push.get() && tool.has(Cap::Push) {
                vec![("Push...".to_string(), Rc::new(move |w: &Window| super::generic::run(w, &wc3, &[Op::Push])))]
            } else {
                Vec::new()
            };
            unsafe { dd.destroy() };
            progress::run(&w, wc2.root(), wc2.root(), cmds, posts, |w, _| super::overlay::refresh_all(w));
            return;
        };
        let staged = repo2.changes(&[], false, false).unwrap_or_default();
        let nb = new_branch.is_active().then(|| new_name.text().trim().to_string()).filter(|s| !s.is_empty());
        let au = set_author.is_active().then(|| author.text().to_string());
        let mut cmds = commit_cmds(repo2.has_commits(), &staged, &checked, &message, amend.is_active(), au.as_deref(), sign.is_active(), nb.as_deref());
        if and_push.get() {
            cmds.push(push_cmd(&repo2, nb.as_deref()));
        }
        let title = repo2.root.clone();
        let r3 = repo2.clone();
        let posts: Vec<progress::Post> = if and_push.get() {
            Vec::new()
        } else {
            vec![("Push...".to_string(), Rc::new(move |w: &Window| super::dialogs::push(w, super::Target::of_repo(&r3), r3.clone())))]
        };
        unsafe { dd.destroy() };
        progress::run(&w, &repo2.root, &title, cmds, posts, |_, _| {});
    });
    d.show_all();
    msg.grab_focus();
    update();
}

/// Pushing the branch after a commit: to where it pushes to, else to "origin" (and from
/// then on tracking it there).
fn push_cmd(repo: &Repo, new_branch: Option<&str>) -> Cmd {
    let branch = new_branch.map(str::to_string).or_else(|| repo.branch()).unwrap_or_else(|| "HEAD".into());
    match repo.upstream(&branch).and_then(|u| u.split_once('/').map(|(r, b)| (r.to_string(), b.to_string()))) {
        Some((remote, b)) => progress::cmd(&["push", "--progress", &remote, &format!("{branch}:{b}")]),
        None => {
            let remote = repo.remotes().first().cloned().unwrap_or_else(|| "origin".into());
            progress::cmd(&["push", "--progress", "-u", &remote, &branch])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(path: &str, status: Status, staged: bool) -> Change {
        Change { path: path.into(), old_path: None, status, staged, added: None, removed: None }
    }

    #[test]
    fn commits_exactly_what_is_ticked() {
        let staged = [ch("a.txt", Status::Modified, true), ch("b.txt", Status::Added, true), ch("c.txt", Status::Modified, false)];
        let checked = [ch("a.txt", Status::Modified, true), ch("c.txt", Status::Modified, false)];
        let cmds = commit_cmds(true, &staged, &checked, "msg", false, None, false, None);
        let shown: Vec<String> = cmds.iter().map(Cmd::display).collect();
        assert_eq!(shown, ["git reset -q -- b.txt", "git add -A -- a.txt c.txt", "git commit -F -"]);
        assert_eq!(cmds[2].stdin.as_deref(), Some("msg"));
    }

    #[test]
    fn options_of_the_commit() {
        let mut renamed = ch("new.txt", Status::Renamed, true);
        renamed.old_path = Some("old.txt".into());
        let cmds = commit_cmds(false, &[ch("x", Status::Added, true)], &[renamed], "m", true, Some("Bob <b@x>"), true, Some("topic"));
        let shown: Vec<String> = cmds.iter().map(Cmd::display).collect();
        assert_eq!(
            shown,
            [
                "git checkout -b topic",
                "git rm --cached -r -q --ignore-unmatch -- x",
                "git add -A -- new.txt old.txt",
                "git commit --amend \"--author=Bob <b@x>\" -s -F -",
            ]
        );
    }

    #[test]
    fn check_links() {
        let u = ch("u", Status::Unversioned, false);
        let m = ch("m", Status::Modified, false);
        let a = ch("a", Status::Added, true);
        assert!(check_filter("Unversioned", &u) && !check_filter("Unversioned", &m));
        assert!(check_filter("Versioned", &m) && !check_filter("Versioned", &u));
        assert!(check_filter("Added", &a) && !check_filter("Added", &m));
        assert!(!check_filter("None", &a) && check_filter("All", &u));
    }
}
