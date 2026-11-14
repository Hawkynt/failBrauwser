//! TortoiseGit's smaller dialogs, each laid out and worded as there: Pull, Fetch, Push,
//! Switch/Checkout, Merge, Create Branch, Create Tag, Reset, Export, Revert, Resolve, Add,
//! Clean up, Stash, Ignore, Rename, Delete, Patch Serial, Clone, Git init, RefLog, Check for
//! modifications ("Working Tree"), Settings and About.

use super::super::window::Window;
use super::Wc;
use super::dialog::{self, BaseOn};
use super::filelist::{self, FileList};
use super::progress::{self, Cmd};
use super::Target;
use failbrauwser::vcs::git::{self, Change, Repo, Status};
use gtk::prelude::*;
use gtk::glib;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Runs the commands in the progress window, from the top of the working tree.
fn run(w: &Window, repo: &Repo, cmds: Vec<Cmd>) {
    progress::run(w, &repo.root, &repo.root, cmds, Vec::new(), |_, _| {});
}

/// Runs a quick git command without a progress window (rename, delete, init), telling
/// only what went wrong; the overlays and the folder show the result.
fn quick(w: &Window, dir: &Path, args: Vec<String>, done: impl FnOnce(&Window) + 'static) {
    let dir = dir.to_path_buf();
    let ww = Rc::downgrade(&w.me());
    super::background(
        move || {
            let mut c = std::process::Command::new("git");
            c.current_dir(&dir).args(&args);
            progress::prepare(&mut c);
            c.stdin(std::process::Stdio::null());
            match c.output() {
                Ok(o) if o.status.success() => Ok(()),
                Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                Err(e) => Err(e.to_string()),
            }
        },
        move |res| {
            let Some(w) = ww.upgrade() else { return };
            if let Err(e) = res {
                dialog::error(&w.win, &e);
            }
            for p in w.panes() {
                p.reload();
            }
            super::overlay::refresh_all(&w);
            done(&w);
        },
    );
}

/// A remote chosen in a "Remote" group box: a configured remote, or an arbitrary URL.
struct RemotePick {
    frame: gtk::Frame,
    remote_radio: gtk::RadioButton,
    remote: gtk::ComboBoxText,
    url: gtk::Entry,
}

impl RemotePick {
    fn new(title: &str, repo: &Repo, with_all: bool, preferred: Option<&str>) -> RemotePick {
        let g = dialog::grid();
        let r = dialog::radios(&[("Remote:", "remote-radio"), ("Arbitrary URL:", "url-radio")]);
        let mut remotes = repo.remotes();
        if with_all && remotes.len() > 1 {
            remotes.push("- All -".into());
        }
        let first = preferred.filter(|p| remotes.iter().any(|r| r == p)).or(remotes.first().map(String::as_str)).unwrap_or("").to_string();
        let remote = dialog::combo("remote", &remotes, &first);
        let url = dialog::entry("url", "");
        g.attach(&r[0], 0, 0, 1, 1);
        remote.set_hexpand(true);
        g.attach(&remote, 1, 0, 1, 1);
        g.attach(&r[1], 0, 1, 1, 1);
        g.attach(&url, 1, 1, 1, 1);
        let r1 = r[1].clone();
        url.connect_changed(move |e| {
            if e.has_focus() {
                r1.set_active(true);
            }
        });
        if remotes.is_empty() {
            r[1].set_active(true);
        }
        RemotePick { frame: dialog::frame(title, &g), remote_radio: r[0].clone(), remote, url }
    }

    /// The remote's name or the URL; None for "- All -".
    fn value(&self) -> Option<String> {
        if self.remote_radio.is_active() {
            let v = dialog::combo_text(&self.remote);
            (v != "- All -").then_some(v)
        } else {
            Some(self.url.text().trim().to_string())
        }
    }
}

/// Branch names of `remote` ("main" for "origin/main").
fn remote_branches(repo: &Repo, remote: &str) -> Vec<String> {
    repo.refs().remote_branches.iter().filter_map(|b| b.strip_prefix(&format!("{remote}/")).map(str::to_string)).collect()
}

/// The branch `branch` pulls from: (remote, branch there).
fn upstream_parts(repo: &Repo, branch: &str) -> Option<(String, String)> {
    let up = repo.upstream(branch)?;
    let remotes = repo.remotes();
    let remote = remotes.iter().find(|r| up.starts_with(&format!("{r}/")))?;
    Some((remote.clone(), up[remote.len() + 1..].to_string()))
}

/// Runs `on_ok` when the dialog is answered with OK (and closes it either way).
fn on_ok(d: &gtk::Dialog, f: impl Fn() -> bool + 'static) {
    d.connect_response(move |d, r| {
        if r == gtk::ResponseType::Ok && !f() {
            return;
        }
        unsafe { d.destroy() };
    });
}

pub fn pull_cmd(remote: &str, branch: &str, squash: bool, no_commit: bool, no_ff: bool, ff_only: bool, prune: bool, rebase: bool) -> Cmd {
    let mut a = vec!["pull", "--progress", "-v"];
    a.push(if rebase { "--rebase" } else { "--no-rebase" });
    for (on, flag) in [(squash, "--squash"), (no_commit, "--no-commit"), (no_ff, "--no-ff"), (ff_only, "--ff-only"), (prune, "--prune")] {
        if on {
            a.push(flag);
        }
    }
    a.push(remote);
    if !branch.is_empty() {
        a.push(branch);
    }
    progress::cmd(&a)
}

pub fn pull(w: &Window, t: Target, repo: Repo) {
    let d = dialog::new(w, &dialog::title(&t.title_path(), "Pull"), "pull");
    let branch = repo.branch().unwrap_or_default();
    let up = upstream_parts(&repo, &branch);
    let pick = RemotePick::new("Remote", &repo, false, up.as_ref().map(|u| u.0.as_str()));
    let area = d.content_area();
    area.pack_start(&pick.frame, false, false, 0);
    let g = dialog::grid();
    let rb_items = remote_branches(&repo, &dialog::combo_text(&pick.remote));
    let rb = dialog::combo("remote-branch", &rb_items, up.as_ref().map(|u| u.1.as_str()).unwrap_or(&branch));
    dialog::grid_row(&g, 0, "Remote Branch:", &rb);
    area.pack_start(&g, false, false, 0);
    let opts = gtk::Grid::new();
    opts.set_column_spacing(24);
    let checks: Vec<gtk::CheckButton> = [("Squash", "squash"), ("No Commit", "no-commit"), ("No Fast Forward", "no-ff"), ("Fast Forward Only", "ff-only"), ("Prune", "prune"), ("Rebase", "rebase")]
        .iter()
        .map(|(l, n)| dialog::check(l, n, false))
        .collect();
    for (i, c) in checks.iter().enumerate() {
        opts.attach(c, (i % 2) as i32, (i / 2) as i32, 1, 1);
    }
    area.pack_start(&dialog::frame("Options", &opts), false, false, 0);
    dialog::ok_cancel(&d);
    let ww = Rc::downgrade(&w.me());
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let Some(remote) = pick.value().filter(|r| !r.is_empty()) else {
            dialog::error(&w.win, "Choose a remote or enter a URL.");
            return false;
        };
        let c = |i: usize| checks[i].is_active();
        let cmd = pull_cmd(&remote, &dialog::combo_text(&rb), c(0), c(1), c(2), c(3), c(4), c(5));
        let r2 = repo.clone();
        let posts: Vec<progress::Post> = vec![("Log".into(), Rc::new(move |w: &Window| super::log::show(w, &super::wc(&r2), &r2.root, Vec::new())))];
        progress::run(&w, &repo.root, &repo.root, vec![cmd], posts, |_, _| {});
        true
    });
    d.show_all();
}

pub fn fetch(w: &Window, t: Target, repo: Repo) {
    let d = dialog::new(w, &dialog::title(&t.title_path(), "Fetch"), "fetch");
    let branch = repo.branch().unwrap_or_default();
    let up = upstream_parts(&repo, &branch);
    let pick = RemotePick::new("Remote", &repo, true, up.as_ref().map(|u| u.0.as_str()));
    let area = d.content_area();
    area.pack_start(&pick.frame, false, false, 0);
    let opts = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let prune = dialog::check("Prune", "prune", false);
    let tags_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    tags_row.pack_start(&dialog::label("Tags:"), false, false, 0);
    let tags = gtk::ComboBoxText::new();
    tags.set_widget_name("tags");
    for (id, l) in [("reachable", "Reachable"), ("all", "Fetch all"), ("none", "None")] {
        tags.append(Some(id), l);
    }
    tags.set_active_id(Some("reachable"));
    tags_row.pack_start(&tags, false, false, 0);
    opts.pack_start(&prune, false, false, 0);
    opts.pack_start(&tags_row, false, false, 0);
    area.pack_start(&dialog::frame("Options", &opts), false, false, 0);
    dialog::ok_cancel(&d);
    let ww = Rc::downgrade(&w.me());
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let mut a = vec!["fetch".to_string(), "--progress".into(), "-v".into()];
        if prune.is_active() {
            a.push("--prune".into());
        }
        match tags.active_id().as_deref() {
            Some("all") => a.push("--tags".into()),
            Some("none") => a.push("--no-tags".into()),
            _ => {}
        }
        match pick.value() {
            Some(r) if !r.is_empty() => a.push(r),
            Some(_) => {
                dialog::error(&w.win, "Choose a remote or enter a URL.");
                return false;
            }
            None => a.push("--all".into()),
        }
        let r2 = repo.clone();
        let posts: Vec<progress::Post> = vec![("Log".into(), Rc::new(move |w: &Window| super::log::show(w, &super::wc(&r2), &r2.root, Vec::new())))];
        progress::run(&w, &repo.root, &repo.root, vec![progress::cmd(&a)], posts, |_, _| {});
        true
    });
    d.show_all();
}

pub fn push_cmd(remote: Option<&str>, local: &str, remote_branch: &str, all: bool, force: Option<bool>, tags: bool, upstream: bool) -> Cmd {
    let mut a: Vec<String> = vec!["push".into(), "--progress".into(), "-v".into()];
    match force {
        Some(true) => a.push("--force".into()),
        Some(false) => a.push("--force-with-lease".into()),
        None => {}
    }
    if tags {
        a.push("--tags".into());
    }
    if upstream {
        a.push("-u".into());
    }
    if all {
        a.push("--all".into());
    }
    a.push(remote.unwrap_or("origin").to_string());
    if !all {
        let rb = if remote_branch.is_empty() { local } else { remote_branch };
        a.push(if rb == local { local.to_string() } else { format!("{local}:{rb}") });
    }
    progress::cmd(&a)
}

pub fn push(w: &Window, t: Target, repo: Repo) {
    let d = dialog::new(w, &dialog::title(&t.title_path(), "Push"), "push");
    let area = d.content_area();
    let refs = repo.refs();
    let branch = repo.branch().unwrap_or_default();
    let up = upstream_parts(&repo, &branch);
    let g = dialog::grid();
    let local = dialog::combo("local", &refs.branches, &branch);
    let rb_items = remote_branches(&repo, up.as_ref().map(|u| u.0.as_str()).or(repo.remotes().first().map(String::as_str)).unwrap_or("origin"));
    let rb = dialog::combo("remote-branch", &rb_items, up.as_ref().map(|u| u.1.as_str()).unwrap_or(&branch));
    dialog::grid_row(&g, 0, "Local:", &local);
    dialog::grid_row(&g, 1, "Remote:", &rb);
    let all = dialog::check("Push all branches", "all-branches", false);
    g.attach(&all, 1, 2, 1, 1);
    area.pack_start(&dialog::frame("Ref", &g), false, false, 0);
    let pick = RemotePick::new("Destination", &repo, false, up.as_ref().map(|u| u.0.as_str()));
    area.pack_start(&pick.frame, false, false, 0);
    let opts = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let force_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let force = dialog::check("Force: May discard", "force", false);
    let kinds = dialog::radios(&[("known changes", "force-known"), ("unknown changes", "force-unknown")]);
    force_row.pack_start(&force, false, false, 0);
    for k in &kinds {
        k.set_sensitive(false);
        force_row.pack_start(k, false, false, 0);
    }
    let ks = kinds.clone();
    force.connect_toggled(move |c| ks.iter().for_each(|k| k.set_sensitive(c.is_active())));
    let tags = dialog::check("Include Tags", "tags", false);
    let track = dialog::check("Set upstream/track remote branch", "set-upstream", up.is_none());
    opts.pack_start(&force_row, false, false, 0);
    opts.pack_start(&tags, false, false, 0);
    opts.pack_start(&track, false, false, 0);
    area.pack_start(&dialog::frame("Options", &opts), false, false, 0);
    dialog::ok_cancel(&d);
    let ww = Rc::downgrade(&w.me());
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let remote = pick.value().filter(|r| !r.is_empty());
        if remote.is_none() {
            dialog::error(&w.win, "Choose a remote or enter a URL.");
            return false;
        }
        let f = force.is_active().then(|| kinds[1].is_active());
        let cmd = push_cmd(remote.as_deref(), &dialog::combo_text(&local), &dialog::combo_text(&rb), all.is_active(), f, tags.is_active(), track.is_active());
        run(&w, &repo, vec![cmd]);
        true
    });
    d.show_all();
}

/// TortoiseGit's "Switch/Checkout": what to check out, a new branch to create, force and
/// track.
pub fn switch(w: &Window, t: Target, repo: Repo) {
    switch_dialog(w, &t.title_path(), &repo, None);
}

pub fn switch_to(w: &Window, repo: &Repo, rev: &str) {
    switch_dialog(w, &repo.root, repo, Some(rev));
}

pub fn switch_cmd(rev: &str, remote_branches: &[String], new_branch: Option<&str>, force: bool, track: bool, override_branch: bool) -> Cmd {
    let mut a: Vec<String> = vec!["checkout".into()];
    if force {
        a.push("-f".into());
    }
    match new_branch {
        Some(b) => {
            a.push(if override_branch { "-B" } else { "-b" }.into());
            a.push(b.into());
            if track {
                a.push("--track".into());
            }
        }
        // A remote branch: a local branch of the same name that tracks it, as TortoiseGit does.
        None if remote_branches.iter().any(|r| r == rev) => {
            a.push("-b".into());
            a.push(rev.split_once('/').map(|x| x.1).unwrap_or(rev).into());
            a.push("--track".into());
        }
        None => {}
    }
    a.push(rev.into());
    progress::cmd(&a)
}

fn switch_dialog(w: &Window, path: &Path, repo: &Repo, rev: Option<&str>) {
    let d = dialog::new(w, &dialog::title(path, "Switch/Checkout"), "switch");
    let refs = repo.refs();
    let mut branches = refs.branches.clone();
    branches.extend(refs.remote_branches.iter().cloned());
    let base = BaseOn::new("Switch To", &branches, &refs.tags, &repo.branch().unwrap_or_default(), false);
    if let Some(r) = rev {
        base.set_commit(r);
    }
    let area = d.content_area();
    area.pack_start(&base.frame, false, false, 0);
    let g = dialog::grid();
    let create = dialog::check("Create New Branch", "create-branch", false);
    let name = dialog::entry("branch-name", "");
    name.set_sensitive(false);
    g.attach(&create, 0, 0, 1, 1);
    name.set_hexpand(true);
    g.attach(&name, 1, 0, 1, 1);
    let n2 = name.clone();
    create.connect_toggled(move |c| n2.set_sensitive(c.is_active()));
    let force = dialog::check("Force", "force", false);
    let track = dialog::check("Track", "track", false);
    let over = dialog::check("Override branch if exists", "override", false);
    g.attach(&force, 0, 1, 1, 1);
    g.attach(&track, 1, 1, 1, 1);
    g.attach(&over, 0, 2, 2, 1);
    area.pack_start(&dialog::frame("Options", &g), false, false, 0);
    dialog::ok_cancel(&d);
    let (ww, repo) = (Rc::downgrade(&w.me()), repo.clone());
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let nb = create.is_active().then(|| name.text().trim().to_string());
        if nb.as_deref() == Some("") {
            dialog::error(&w.win, "Enter a name for the new branch.");
            return false;
        }
        let cmd = switch_cmd(&base.rev(), &refs.remote_branches, nb.as_deref(), force.is_active(), track.is_active(), over.is_active());
        run(&w, &repo, vec![cmd]);
        true
    });
    d.show_all();
}

pub fn merge(w: &Window, t: Target, repo: Repo) {
    let d = dialog::new(w, &dialog::title(&t.title_path(), "Merge"), "merge");
    let refs = repo.refs();
    let current = repo.branch().unwrap_or_default();
    let mut branches: Vec<String> = refs.branches.iter().filter(|b| **b != current).cloned().collect();
    branches.extend(refs.remote_branches.iter().cloned());
    let base = BaseOn::new("From", &branches, &refs.tags, "", false);
    let area = d.content_area();
    area.pack_start(&base.frame, false, false, 0);
    let opts = gtk::Grid::new();
    opts.set_column_spacing(24);
    let checks: Vec<gtk::CheckButton> = [("Squash", "squash"), ("No Fast Forward", "no-ff"), ("Fast Forward Only", "ff-only"), ("No Commit", "no-commit")].iter().map(|(l, n)| dialog::check(l, n, false)).collect();
    for (i, c) in checks.iter().enumerate() {
        opts.attach(c, (i % 2) as i32, (i / 2) as i32, 1, 1);
    }
    area.pack_start(&dialog::frame("Option", &opts), false, false, 0);
    let (sw, msg) = dialog::text_view("message", true);
    sw.set_size_request(-1, 90);
    area.pack_start(&dialog::frame("Merge Message", &sw), true, true, 0);
    dialog::ok_cancel(&d);
    let ww = Rc::downgrade(&w.me());
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let mut a: Vec<String> = vec!["merge".into()];
        for (c, flag) in checks.iter().zip(["--squash", "--no-ff", "--ff-only", "--no-commit"]) {
            if c.is_active() {
                a.push(flag.into());
            }
        }
        let m = dialog::buffer_text(&msg);
        if !m.trim().is_empty() {
            a.push("-m".into());
            a.push(m.trim().to_string());
        }
        a.push(base.rev());
        run(&w, &repo, vec![progress::cmd(&a)]);
        true
    });
    d.show_all();
}

pub fn branch(w: &Window, t: Target, repo: Repo) {
    branch_dialog(w, &t.title_path(), &repo, None);
}

pub fn branch_at(w: &Window, repo: &Repo, rev: &str) {
    branch_dialog(w, &repo.root, repo, Some(rev));
}

fn branch_dialog(w: &Window, path: &Path, repo: &Repo, rev: Option<&str>) {
    let d = dialog::new(w, &dialog::title(path, "Create Branch"), "branch");
    let area = d.content_area();
    let g = dialog::grid();
    let name = dialog::entry("branch-name", "");
    dialog::grid_row(&g, 0, "Branch:", &name);
    area.pack_start(&dialog::frame("Branch", &g), false, false, 0);
    let refs = repo.refs();
    let mut branches = refs.branches.clone();
    branches.extend(refs.remote_branches.iter().cloned());
    let base = BaseOn::new("Base On", &branches, &refs.tags, &repo.branch().unwrap_or_default(), true);
    if let Some(r) = rev {
        base.set_commit(r);
    }
    area.pack_start(&base.frame, false, false, 0);
    let opts = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let track = dialog::check("Track", "track", false);
    let force = dialog::check("Force", "force", false);
    let switch = dialog::check("Switch to new branch", "switch", false);
    for c in [&track, &force, &switch] {
        opts.pack_start(c, false, false, 0);
    }
    area.pack_start(&dialog::frame("Options", &opts), false, false, 0);
    dialog::ok_cancel(&d);
    let (ww, repo) = (Rc::downgrade(&w.me()), repo.clone());
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let n = name.text().trim().to_string();
        if n.is_empty() {
            dialog::error(&w.win, "Enter a name for the branch.");
            return false;
        }
        let mut a: Vec<String> = vec!["branch".into()];
        if track.is_active() {
            a.push("--track".into());
        }
        if force.is_active() {
            a.push("-f".into());
        }
        a.push(n.clone());
        a.push(base.rev());
        let mut cmds = vec![progress::cmd(&a)];
        if switch.is_active() {
            cmds.push(progress::cmd(&["checkout", &n]));
        }
        run(&w, &repo, cmds);
        true
    });
    d.show_all();
}

pub fn tag(w: &Window, t: Target, repo: Repo) {
    tag_dialog(w, &t.title_path(), &repo, None);
}

pub fn tag_at(w: &Window, repo: &Repo, rev: &str) {
    tag_dialog(w, &repo.root, repo, Some(rev));
}

fn tag_dialog(w: &Window, path: &Path, repo: &Repo, rev: Option<&str>) {
    let d = dialog::new(w, &dialog::title(path, "Create Tag"), "tag");
    let area = d.content_area();
    let g = dialog::grid();
    let name = dialog::entry("tag-name", "");
    dialog::grid_row(&g, 0, "Tag:", &name);
    area.pack_start(&dialog::frame("Tag", &g), false, false, 0);
    let refs = repo.refs();
    let base = BaseOn::new("Base On", &refs.branches, &refs.tags, &repo.branch().unwrap_or_default(), true);
    if let Some(r) = rev {
        base.set_commit(r);
    }
    area.pack_start(&base.frame, false, false, 0);
    let opts = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let force = dialog::check("Force", "force", false);
    let sign = dialog::check("Sign", "sign", false);
    opts.pack_start(&force, false, false, 0);
    opts.pack_start(&sign, false, false, 0);
    area.pack_start(&dialog::frame("Options", &opts), false, false, 0);
    let (sw, msg) = dialog::text_view("message", true);
    sw.set_size_request(-1, 90);
    area.pack_start(&dialog::frame("Message", &sw), true, true, 0);
    dialog::ok_cancel(&d);
    let (ww, repo) = (Rc::downgrade(&w.me()), repo.clone());
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let n = name.text().trim().to_string();
        if n.is_empty() {
            dialog::error(&w.win, "Enter a name for the tag.");
            return false;
        }
        let m = dialog::buffer_text(&msg);
        let mut a: Vec<String> = vec!["tag".into()];
        if force.is_active() {
            a.push("-f".into());
        }
        if sign.is_active() {
            a.push("-s".into());
        }
        let annotated = !m.trim().is_empty() || sign.is_active();
        if annotated {
            a.extend(["-a".into(), "-F".into(), "-".into()]);
        }
        a.push(n);
        a.push(base.rev());
        let mut c = progress::cmd(&a);
        if annotated {
            c = c.input(&m);
        }
        run(&w, &repo, vec![c]);
        true
    });
    d.show_all();
}

/// "Reset "main" to this": soft, mixed or hard.
pub fn reset_to(w: &Window, repo: &Repo, rev: &str) {
    let d = dialog::new(w, &dialog::title(&repo.root, "Reset"), "revert");
    let area = d.content_area();
    let short = &rev[..rev.len().min(8)];
    area.pack_start(&dialog::label(&format!("Reset {} to {short}", repo.branch_label())), false, false, 0);
    let kinds = dialog::radios(&[
        ("Soft: Leave working tree and index untouched", "soft"),
        ("Mixed: Leave working tree untouched, reset index", "mixed"),
        ("Hard: Reset working tree and index (discard all local changes)", "hard"),
    ]);
    kinds[1].set_active(true);
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    for k in &kinds {
        b.pack_start(k, false, false, 0);
    }
    area.pack_start(&dialog::frame("Reset Type", &b), false, false, 0);
    dialog::ok_cancel(&d);
    let (ww, repo, rev) = (Rc::downgrade(&w.me()), repo.clone(), rev.to_string());
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let mode = ["--soft", "--mixed", "--hard"][kinds.iter().position(|k| k.is_active()).unwrap_or(1)];
        run(&w, &repo, vec![progress::cmd(&["reset", mode, &rev])]);
        true
    });
    d.show_all();
}

pub fn export(w: &Window, t: Target, repo: Repo) {
    let d = dialog::new(w, &dialog::title(&t.title_path(), "Export"), "export");
    let area = d.content_area();
    let name = repo.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "export".into());
    let default = repo.root.parent().unwrap_or(&repo.root).join(format!("{name}.zip"));
    let g = dialog::grid();
    let file = dialog::entry("zip-file", &default.to_string_lossy());
    let browse = gtk::Button::with_label("...");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.pack_start(&file, true, true, 0);
    row.pack_start(&browse, false, false, 0);
    dialog::grid_row(&g, 0, "Zip file:", &row);
    area.pack_start(&g, false, false, 0);
    let refs = repo.refs();
    let base = BaseOn::new("Version", &refs.branches, &refs.tags, &repo.branch().unwrap_or_default(), true);
    area.pack_start(&base.frame, false, false, 0);
    let (f2, dd) = (file.clone(), d.clone());
    browse.connect_clicked(move |_| {
        let c = gtk::FileChooserDialog::with_buttons(Some("Export"), Some(&dd), gtk::FileChooserAction::Save, &[("Cancel", gtk::ResponseType::Cancel), ("Save", gtk::ResponseType::Accept)]);
        c.set_current_name(&Path::new(&f2.text().to_string()).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        let f3 = f2.clone();
        c.connect_response(move |c, r| {
            if r == gtk::ResponseType::Accept {
                if let Some(p) = c.filename() {
                    f3.set_text(&p.to_string_lossy());
                }
            }
            unsafe { c.destroy() };
        });
        c.show();
    });
    dialog::ok_cancel(&d);
    let ww = Rc::downgrade(&w.me());
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let out = file.text().trim().to_string();
        if out.is_empty() {
            return false;
        }
        run(&w, &repo, vec![progress::cmd(&["archive", "--format=zip", &format!("--output={out}"), &base.rev()])]);
        true
    });
    d.show_all();
}

/// The commands that take changes back: added files out of the index (the file stays,
/// unversioned), everything else as HEAD has it.
pub fn revert_cmds(has_commits: bool, files: &[Change]) -> Vec<Cmd> {
    let mut uncache: Vec<&str> = Vec::new();
    let mut restore: Vec<&str> = Vec::new();
    for c in files {
        match c.status {
            Status::Added => uncache.push(&c.path),
            Status::Renamed => {
                uncache.push(&c.path);
                restore.extend(c.old_path.as_deref());
            }
            Status::Unversioned | Status::Ignored | Status::Normal => {}
            _ if !has_commits => uncache.push(&c.path),
            _ => restore.push(&c.path),
        }
    }
    let mut out = Vec::new();
    if !uncache.is_empty() {
        let mut a = vec!["rm", "--cached", "-r", "-q", "-f", "--"];
        a.extend(uncache);
        out.push(progress::cmd(&a));
    }
    if !restore.is_empty() {
        let mut a = vec!["checkout", "HEAD", "--"];
        a.extend(restore);
        out.push(progress::cmd(&a));
    }
    out
}

/// A list dialog of files to tick (Revert, Add, Resolve): the list, "Select / deselect
/// all" and OK with what is ticked.
fn pick_files(w: &Window, title: &str, icon: &str, intro: &str, changes: Vec<Change>, checked: bool, ok: impl Fn(&Window, Vec<Change>) + 'static) -> (gtk::Dialog, Rc<FileList>) {
    let d = dialog::new(w, title, icon);
    d.set_default_size(640, 420);
    let area = d.content_area();
    area.pack_start(&dialog::label(intro), false, false, 0);
    let list = FileList::new("files", true);
    area.pack_start(&list.scroll, true, true, 0);
    let all = dialog::check("Select / deselect all", "select-all", checked);
    area.pack_start(&all, false, false, 0);
    let lw = Rc::downgrade(&list);
    all.connect_toggled(move |c| {
        if let Some(l) = lw.upgrade() {
            let on = c.is_active();
            l.check_where(|_| on);
        }
    });
    list.set(changes, |_| checked);
    dialog::ok_cancel(&d);
    let (ww, lw) = (Rc::downgrade(&w.me()), Rc::downgrade(&list));
    on_ok(&d, move || {
        let (Some(w), Some(l)) = (ww.upgrade(), lw.upgrade()) else { return true };
        let files = l.checked();
        if files.is_empty() {
            return true;
        }
        ok(&w, files);
        true
    });
    d.show_all();
    (d, list)
}

pub fn revert(w: &Window, t: Target, repo: Repo) {
    let ww = Rc::downgrade(&w.me());
    let (r, paths) = (repo.clone(), t.rel());
    let title = dialog::title(&t.title_path(), "Revert");
    super::background(move || r.changes(&paths, false, false).unwrap_or_default(), move |changes| {
        let Some(w) = ww.upgrade() else { return };
        let changes: Vec<Change> = changes.into_iter().filter(|c| c.status.is_change()).collect();
        let repo2 = repo.clone();
        pick_files(&w, &title, "revert", "Revert", changes, true, move |w, files| run(w, &repo2, revert_cmds(repo2.has_commits(), &files)));
    });
}

/// Reverts these files after asking (from a file list's right-click menu).
pub fn revert_files(w: &Window, repo: &Repo, files: &[Change]) {
    let (ww, repo, files) = (Rc::downgrade(&w.me()), repo.clone(), files.to_vec());
    let what = if files.len() == 1 { files[0].path.clone() } else { format!("{} files", files.len()) };
    dialog::confirm(&w.win, &format!("Do you really want to revert {what}?"), "The changes are lost.", "Revert", move || {
        if let Some(w) = ww.upgrade() {
            run(&w, &repo, revert_cmds(repo.has_commits(), &files));
        }
    });
}

pub fn add(w: &Window, t: Target, repo: Repo) {
    let ww = Rc::downgrade(&w.me());
    let (r, paths) = (repo.clone(), t.rel());
    let title = dialog::title(&t.title_path(), "Add");
    super::background(move || r.changes(&paths, true, false).unwrap_or_default(), move |changes| {
        let Some(w) = ww.upgrade() else { return };
        let changes: Vec<Change> = changes.into_iter().filter(|c| c.status == Status::Unversioned).collect();
        let repo2 = repo.clone();
        pick_files(&w, &title, "add", "Add", changes, true, move |w, files| {
            let mut a = vec!["add".to_string(), "--".into()];
            a.extend(files.into_iter().map(|c| c.path));
            run(w, &repo2, vec![progress::cmd(&a)]);
        });
    });
}

pub fn resolve(w: &Window, t: Target, repo: Repo) {
    let ww = Rc::downgrade(&w.me());
    let (r, paths) = (repo.clone(), t.rel());
    let title = dialog::title(&t.title_path(), "Resolve");
    super::background(move || r.changes(&paths, false, false).unwrap_or_default(), move |changes| {
        let Some(w) = ww.upgrade() else { return };
        let changes: Vec<Change> = changes.into_iter().filter(|c| c.status == Status::Conflicted).collect();
        let repo2 = repo.clone();
        let (_, list) = pick_files(&w, &title, "resolve", "Mark these conflicts as resolved:", changes, true, move |w, files| {
            let mut a = vec!["add".to_string(), "--".into()];
            a.extend(files.into_iter().map(|c| c.path));
            run(w, &repo2, vec![progress::cmd(&a)]);
        });
        let (ww, r3) = (Rc::downgrade(&w.me()), repo.clone());
        list.connect_activated(move |c| {
            if let Some(w) = ww.upgrade() {
                super::diff::working(&w, &super::wc(&r3), &c.path);
            }
        });
    });
}

pub fn cleanup_cmd(kind: usize, dirs: bool, dry: bool, paths: &[String]) -> Cmd {
    let mut a: Vec<String> = vec!["clean".into(), "-f".into()];
    if dirs {
        a.push("-d".into());
    }
    match kind {
        1 => a.push("-X".into()),
        2 => a.push("-x".into()),
        _ => {}
    }
    if dry {
        a.push("-n".into());
    }
    a.push("--".into());
    a.extend(paths.iter().map(|p| if p.is_empty() { ".".to_string() } else { p.clone() }));
    progress::cmd(&a)
}

pub fn cleanup(w: &Window, t: Target, repo: Repo) {
    let d = dialog::new(w, &dialog::title(&t.title_path(), "Clean up"), "cleanup");
    let area = d.content_area();
    let kinds = dialog::radios(&[("Remove untracked files", "untracked"), ("Remove ignored files", "ignored"), ("Remove untracked and ignored files", "both")]);
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    for k in &kinds {
        b.pack_start(k, false, false, 0);
    }
    area.pack_start(&dialog::frame("Clean Type", &b), false, false, 0);
    let opts = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let dirs = dialog::check("Include directories", "dirs", true);
    let dry = dialog::check("Dry run", "dry-run", false);
    opts.pack_start(&dirs, false, false, 0);
    opts.pack_start(&dry, false, false, 0);
    area.pack_start(&dialog::frame("Options", &opts), false, false, 0);
    dialog::ok_cancel(&d);
    let ww = Rc::downgrade(&w.me());
    let paths = t.rel();
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let kind = kinds.iter().position(|k| k.is_active()).unwrap_or(0);
        run(&w, &repo, vec![cleanup_cmd(kind, dirs.is_active(), dry.is_active(), &paths)]);
        true
    });
    d.show_all();
}

pub fn stash_save(w: &Window, t: Target, repo: Repo) {
    let d = dialog::new(w, &dialog::title(&t.title_path(), "Stash"), "stashsave");
    let area = d.content_area();
    let g = dialog::grid();
    let msg = dialog::entry("message", "");
    dialog::grid_row(&g, 0, "Message:", &msg);
    area.pack_start(&g, false, false, 0);
    let opts = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let untracked = dialog::check("Include untracked", "untracked", false);
    let all = dialog::check("--all", "all", false);
    opts.pack_start(&untracked, false, false, 0);
    opts.pack_start(&all, false, false, 0);
    area.pack_start(&dialog::frame("Options", &opts), false, false, 0);
    dialog::ok_cancel(&d);
    let ww = Rc::downgrade(&w.me());
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let mut a: Vec<String> = vec!["stash".into(), "push".into()];
        if all.is_active() {
            a.push("--all".into());
        } else if untracked.is_active() {
            a.push("--include-untracked".into());
        }
        let m = msg.text().trim().to_string();
        if !m.is_empty() {
            a.push("-m".into());
            a.push(m);
        }
        run(&w, &repo, vec![progress::cmd(&a)]);
        true
    });
    d.show_all();
}

pub fn stash_pop(w: &Window, _t: Target, repo: Repo) {
    run(w, &repo, vec![progress::cmd(&["stash", "pop"])]);
}

pub fn delete(w: &Window, t: Target, repo: Repo, keep: bool) {
    let rel = t.rel();
    let what = match t.paths.as_slice() {
        [one] => one.display().to_string(),
        many => format!("{} items", many.len()),
    };
    let ww = Rc::downgrade(&w.me());
    let text = if keep { format!("Remove {what} from the repository and keep it on disk?") } else { format!("Do you really want to delete {what}?") };
    dialog::confirm(&w.win, &text, "", "Delete", move || {
        let Some(w) = ww.upgrade() else { return };
        let mut a: Vec<String> = vec!["rm".into(), "-r".into(), "-q".into()];
        if keep {
            a.push("--cached".into());
        }
        a.push("--".into());
        a.extend(rel.iter().cloned());
        quick(&w, &repo.root, a, |_| {});
    });
}

pub fn rename(w: &Window, t: Target, repo: Repo) {
    let Some(path) = t.paths.first().cloned() else { return };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let d = dialog::new(w, &dialog::title(&path, "Rename"), "rename");
    let area = d.content_area();
    let g = dialog::grid();
    let e = dialog::entry("new-name", &name);
    e.set_width_chars(40);
    let e2 = e.clone();
    dialog::grid_row(&g, 0, "New name:", &e);
    area.pack_start(&g, false, false, 0);
    dialog::ok_cancel(&d);
    let ww = Rc::downgrade(&w.me());
    let name2 = name.clone();
    on_ok(&d, move || {
        let name = &name2;
        let Some(w) = ww.upgrade() else { return true };
        let new = e.text().trim().to_string();
        if new.is_empty() || new == *name || new.contains('/') {
            return new == *name;
        }
        let (Some(from), Some(to)) = (repo.relative(&path), path.parent().and_then(|p| repo.relative(&p.join(&new)))) else { return true };
        let sel = new.clone();
        quick(&w, &repo.root, vec!["mv".into(), "--".into(), from, to], move |w| w.current_pane().select_when_present(vec![sel.clone().into()]));
        true
    });
    d.show_all();
    // The name without its extension selected, as for renames elsewhere.
    let stem = name.char_indices().filter(|(i, c)| *c == '.' && *i > 0).map(|(i, _)| name[..i].chars().count() as i32).next().unwrap_or(-1);
    e2.grab_focus();
    e2.select_region(0, stem);
}

/// Where TortoiseGit writes an ignore pattern and the line: (ignore file relative to the
/// top of the tree, line). `kind`: 0 the top folder's .gitignore, 1 the item's folder's,
/// 2 .git/info/exclude.
pub fn ignore_line(rel_dir: &str, pattern: &str, recursive: bool, kind: usize) -> (String, String) {
    let here = if kind == 1 || rel_dir.is_empty() { String::new() } else { format!("/{rel_dir}") };
    let line = if recursive && kind == 1 {
        pattern.to_string()
    } else if recursive {
        if rel_dir.is_empty() { pattern.to_string() } else { format!("{here}/**/{pattern}") }
    } else {
        format!("{here}/{pattern}")
    };
    let file = match kind {
        1 if !rel_dir.is_empty() => format!("{rel_dir}/.gitignore"),
        2 => "\0exclude".into(),
        _ => ".gitignore".into(),
    };
    (file, line)
}

pub fn ignore(w: &Window, t: Target, repo: Repo, pattern: String) {
    let d = dialog::new(w, &dialog::title(&t.title_path(), "Ignore"), "ignore");
    let area = d.content_area();
    let types = dialog::radios(&[("Ignore item(s) only in containing folder(s)", "only-here"), ("Ignore item(s) recursively", "recursive")]);
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    types.iter().for_each(|r| b.pack_start(r, false, false, 0));
    area.pack_start(&dialog::frame("Ignore Type", &b), false, false, 0);
    let files = dialog::radios(&[(".gitignore in the repository root", "root"), (".gitignore in the containing directories of the items", "containing"), (".git/info/exclude", "exclude")]);
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    files.iter().for_each(|r| b.pack_start(r, false, false, 0));
    area.pack_start(&dialog::frame("Ignore file", &b), false, false, 0);
    dialog::ok_cancel(&d);
    let ww = Rc::downgrade(&w.me());
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let recursive = types[1].is_active();
        let kind = files.iter().position(|r| r.is_active()).unwrap_or(0);
        for p in &t.paths {
            let (Some(rel_dir), Some(name)) = (p.parent().and_then(|d| repo.relative(d)), p.file_name()) else { continue };
            let pat = if pattern.is_empty() { name.to_string_lossy().into_owned() } else { pattern.clone() };
            let (file, line) = ignore_line(&rel_dir, &pat, recursive, kind);
            let target = if file == "\0exclude" { repo.common_dir().join("info/exclude") } else { repo.root.join(file) };
            if let Err(e) = git::add_ignore(&target, &line) {
                dialog::error(&w.win, &format!("{}: {e}", target.display()));
            }
        }
        for p in w.panes() {
            p.reload();
        }
        super::overlay::refresh_all(&w);
        true
    });
    d.show_all();
}

pub fn patch_create(w: &Window, t: Target, repo: Repo) {
    let d = dialog::new(w, &dialog::title(&t.title_path(), "Create Patch Serial"), "patchcreate");
    let area = d.content_area();
    let g = dialog::grid();
    let dir = dialog::entry("directory", &t.folder.to_string_lossy());
    dialog::grid_row(&g, 0, "Directory:", &dir);
    area.pack_start(&g, false, false, 0);
    let rg = dialog::grid();
    let r = dialog::radios(&[("Number:", "by-number"), ("Since:", "by-since")]);
    let n = gtk::SpinButton::with_range(1.0, 9999.0, 1.0);
    n.set_widget_name("number");
    let refs = repo.refs();
    let mut since_items = refs.remote_branches.clone();
    since_items.extend(refs.branches.iter().cloned());
    since_items.extend(refs.tags.iter().cloned());
    let since = dialog::combo("since", &since_items, since_items.first().map(String::as_str).unwrap_or(""));
    rg.attach(&r[0], 0, 0, 1, 1);
    rg.attach(&n, 1, 0, 1, 1);
    rg.attach(&r[1], 0, 1, 1, 1);
    since.set_hexpand(true);
    rg.attach(&since, 1, 1, 1, 1);
    area.pack_start(&dialog::frame("Range", &rg), false, false, 0);
    dialog::ok_cancel(&d);
    let ww = Rc::downgrade(&w.me());
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let out = dir.text().trim().to_string();
        let range = if r[0].is_active() { format!("-{}", n.value_as_int()) } else { dialog::combo_text(&since) };
        run(&w, &repo, vec![progress::cmd(&["format-patch", "-o", &out, &range])]);
        true
    });
    d.show_all();
}

pub fn patch_apply(w: &Window, t: Target, repo: Repo) {
    let c = gtk::FileChooserDialog::with_buttons(Some("Apply Patch Serial"), Some(&w.win), gtk::FileChooserAction::Open, &[("Cancel", gtk::ResponseType::Cancel), ("Apply", gtk::ResponseType::Accept)]);
    c.set_select_multiple(true);
    let _ = c.set_current_folder(&t.folder);
    let f = gtk::FileFilter::new();
    f.set_name(Some("Patch files"));
    f.add_pattern("*.patch");
    f.add_pattern("*.diff");
    c.add_filter(f);
    let ww = Rc::downgrade(&w.me());
    c.connect_response(move |c, r| {
        let files: Vec<PathBuf> = c.filenames();
        unsafe { c.destroy() };
        let Some(w) = ww.upgrade() else { return };
        if r != gtk::ResponseType::Accept || files.is_empty() {
            return;
        }
        let mut a = vec!["am".to_string(), "--3way".into()];
        a.extend(files.iter().map(|p| p.to_string_lossy().into_owned()));
        run(&w, &repo, vec![progress::cmd(&a)]);
    });
    c.show();
}

pub fn clone_cmd(url: &str, dir: &str, depth: Option<i32>, recursive: bool, branch: Option<&str>, no_checkout: bool, bare: bool) -> Cmd {
    let mut a: Vec<String> = vec!["clone".into(), "--progress".into(), "-v".into()];
    if let Some(d) = depth {
        a.push("--depth".into());
        a.push(d.to_string());
    }
    if recursive {
        a.push("--recursive".into());
    }
    if let Some(b) = branch.filter(|b| !b.is_empty()) {
        a.push("--branch".into());
        a.push(b.into());
    }
    if no_checkout {
        a.push("--no-checkout".into());
    }
    if bare {
        a.push("--bare".into());
    }
    a.push(url.into());
    a.push(dir.into());
    progress::cmd(&a)
}

/// `git init` in `path` (bare where asked), and TortoiseGit's word that it is there.
pub fn init_here(w: &Window, path: &Path, bare: bool) {
    let mut a = vec!["init".to_string()];
    if bare {
        a.push("--bare".into());
    }
    let p = path.to_path_buf();
    quick(w, path, a, move |w| {
        if git::repo_at(&p).is_some() || p.join("HEAD").is_file() {
            let m = gtk::MessageDialog::new(Some(&w.win), gtk::DialogFlags::DESTROY_WITH_PARENT, gtk::MessageType::Info, gtk::ButtonsType::Ok, &format!("Empty Git repository created in {}", p.display()));
            m.set_title(dialog::SUFFIX);
            m.connect_response(|m, _| m.close());
            m.show();
        }
    });
}

/// TortoiseGit's RefLog: what HEAD pointed at, newest first.
pub fn reflog(w: &Window, t: Target, repo: Repo) {
    let d = dialog::new(w, &dialog::title(&t.title_path(), "RefLog"), "reflog");
    d.set_default_size(760, 480);
    let area = d.content_area();
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top.pack_start(&dialog::label("Ref:"), false, false, 0);
    top.pack_start(&dialog::label("HEAD"), false, false, 0);
    area.pack_start(&top, false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let view = gtk::TreeView::with_model(&store);
    view.set_widget_name("reflog");
    for (i, t) in ["Ref", "Action", "Message", "Date"].iter().enumerate() {
        let c = gtk::TreeViewColumn::new();
        c.set_title(t);
        c.set_resizable(true);
        let r = gtk::CellRendererText::new();
        r.set_ellipsize(gtk::pango::EllipsizeMode::End);
        TreeViewColumnExt::pack_start(&c, &r, true);
        TreeViewColumnExt::add_attribute(&c, &r, "text", i as i32);
        if i == 2 {
            c.set_expand(true);
        }
        view.append_column(&c);
    }
    let sw = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.set_vexpand(true);
    sw.add(&view);
    area.pack_start(&sw, true, true, 0);
    for e in repo.reflog().unwrap_or_default() {
        let it = store.append();
        store.set(&it, &[(0, &e.selector), (1, &e.action), (2, &e.message), (3, &dialog::when(e.time)), (4, &e.hash)]);
    }
    let (ww, r2) = (Rc::downgrade(&w.me()), repo.clone());
    view.connect_button_press_event(move |view, ev| {
        if ev.button() != 3 {
            return glib::Propagation::Proceed;
        }
        let (x, y) = ev.position();
        let Some((Some(p), ..)) = view.path_at_pos(x as i32, y as i32) else { return glib::Propagation::Stop };
        view.selection().select_path(&p);
        let Some(m) = view.model() else { return glib::Propagation::Stop };
        let Some(it) = m.iter(&p) else { return glib::Propagation::Stop };
        let hash: String = m.value(&it, 4).get().unwrap_or_default();
        let menu = gtk::Menu::new();
        let entries: [(&str, &str, fn(&Window, &Repo, &str)); 3] =
            [("Switch/Checkout to this...", "switch", switch_to), ("Create Branch at this version...", "branch", branch_at), ("Reset to this...", "revert", reset_to)];
        for (label, icon, f) in entries {
            let mi = filelist::menu_item(label, icon);
            let (ww, r, h) = (ww.clone(), r2.clone(), hash.clone());
            mi.connect_activate(move |_| {
                if let Some(w) = ww.upgrade() {
                    f(&w, &r, &h);
                }
            });
            menu.append(&mi);
        }
        menu.set_attach_widget(Some(view));
        menu.show_all();
        menu.popup_at_pointer(Some(ev));
        glib::Propagation::Stop
    });
    d.add_button("OK", gtk::ResponseType::Ok);
    d.connect_response(|d, _| unsafe { d.destroy() });
    d.show_all();
}

/// TortoiseGit's "Check for Modifications" ("Working Tree"): the changed files, with
/// unversioned and ignored ones when asked; double-click for the diff.
pub fn check_modifications(w: &Window, t: Target, wc: Wc) {
    let d = dialog::new(w, &dialog::title(&t.title_path(), "Working Tree"), "checkmods");
    d.set_default_size(760, 480);
    let area = d.content_area();
    let list = FileList::new("files", false);
    area.pack_start(&list.scroll, true, true, 0);
    let summary = dialog::label("");
    summary.set_widget_name("summary");
    area.pack_start(&summary, false, false, 0);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let unv = dialog::check("Show Unversioned Files", "show-unversioned", true);
    let ign = dialog::check("Show Ignored Files", "show-ignored", false);
    let whole = dialog::check("Show Whole Project", "whole-project", t.rel().iter().any(String::is_empty));
    for c in [&unv, &ign, &whole] {
        row.pack_start(c, false, false, 0);
    }
    area.pack_start(&row, false, false, 0);
    let refresh = d.add_button("Refresh", gtk::ResponseType::Apply).downcast::<gtk::Button>().expect("button");
    d.add_button("OK", gtk::ResponseType::Ok);
    let paths = t.rel();
    let reload = {
        let (lw, wc2, unv, ign, whole, summary) = (Rc::downgrade(&list), wc.clone(), unv.clone(), ign.clone(), whole.clone(), summary.clone());
        Rc::new(move || {
            let (r, u, i) = (wc2.clone(), unv.is_active(), ign.is_active());
            let p = if whole.is_active() { Vec::new() } else { paths.clone() };
            let (lw, summary) = (lw.clone(), summary.clone());
            super::background(move || r.changes(&p, u, i).unwrap_or_default(), move |changes| {
                let Some(l) = lw.upgrade() else { return };
                let count = |s: Status| changes.iter().filter(|c| c.status == s).count();
                summary.set_text(&format!(
                    "{} Modified, {} Added, {} Deleted, {} Unversioned, {} Conflicted",
                    count(Status::Modified),
                    count(Status::Added) + count(Status::Renamed),
                    count(Status::Deleted),
                    count(Status::Unversioned),
                    count(Status::Conflicted)
                ));
                l.set(changes, |_| false);
            });
        })
    };
    for c in [&unv, &ign, &whole] {
        let r = reload.clone();
        c.connect_toggled(move |_| r());
    }
    let r = reload.clone();
    refresh.connect_clicked(move |_| r());
    d.connect_response(|d, r| {
        if r == gtk::ResponseType::Ok || r == gtk::ResponseType::DeleteEvent {
            unsafe { d.destroy() };
        }
    });
    let (ww, r2) = (Rc::downgrade(&w.me()), wc.clone());
    list.connect_activated(move |c| {
        if let Some(w) = ww.upgrade() {
            if c.status.is_change() {
                super::diff::working(&w, &r2, &c.path);
            }
        }
    });
    let (ww, r2) = (Rc::downgrade(&w.me()), wc.clone());
    list.connect_menu(move |sel| {
        let m = gtk::Menu::new();
        let first = sel.first()?.clone();
        if first.status.is_change() {
            let i = filelist::menu_item("Compare with base", "diff");
            let (w1, r, c) = (ww.clone(), r2.clone(), first.clone());
            i.connect_activate(move |_| {
                if let Some(w) = w1.upgrade() {
                    super::diff::working(&w, &r, &c.path);
                }
            });
            m.append(&i);
            let i = filelist::menu_item("Revert...", "revert");
            let (w1, r, files) = (ww.clone(), r2.clone(), sel.to_vec());
            i.connect_activate(move |_| {
                if let Some(w) = w1.upgrade() {
                    super::revert_files(&w, &r, &files);
                }
            });
            m.append(&i);
        }
        if sel.iter().all(|c| c.status == Status::Unversioned) {
            let i = filelist::menu_item("Add...", "add");
            let (w1, r, files) = (ww.clone(), r2.clone(), sel.to_vec());
            i.connect_activate(move |_| {
                if let Some(w) = w1.upgrade() {
                    super::generic::run(&w, &r, &[failbrauwser::vcs::ops::Op::Add { paths: files.iter().map(|c| c.path.clone()).collect() }]);
                }
            });
            m.append(&i);
        }
        let i = filelist::menu_item("Show log", "log");
        let (w1, r, c) = (ww.clone(), r2.clone(), first);
        i.connect_activate(move |_| {
            if let Some(w) = w1.upgrade() {
                super::log::show(&w, &r, &r.root().join(&c.path), vec![c.path.clone()]);
            }
        });
        m.append(&i);
        Some(m)
    });
    let r = reload.clone();
    d.connect_is_active_notify(move |d| {
        if d.is_active() {
            r();
        }
    });
    d.show_all();
    reload();
}

/// TortoiseGit's Settings: "Git" (the user's name, email and signing key, for this
/// repository or for all) and "Icon Overlays".
pub fn settings(w: &Window, repo: Option<Repo>) {
    let d = dialog::new(w, &format!("Settings - {}", dialog::SUFFIX), "settings");
    d.set_default_size(640, 400);
    let area = d.content_area();
    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    let stack = gtk::Stack::new();
    let side = gtk::StackSidebar::new();
    side.set_stack(&stack);
    side.set_size_request(150, -1);
    paned.pack1(&side, false, false);
    paned.pack2(&stack, true, false);
    area.pack_start(&paned, true, true, 0);

    // Git
    let page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.set_border_width(8);
    let src = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    src.pack_start(&dialog::label("Config source:"), false, false, 0);
    let scope = dialog::radios(&[("Global", "scope-global"), ("Local", "scope-local")]);
    scope[1].set_sensitive(repo.is_some());
    scope.iter().for_each(|r| src.pack_start(r, false, false, 0));
    page.pack_start(&src, false, false, 0);
    let g = dialog::grid();
    let name = dialog::entry("user-name", "");
    let email = dialog::entry("user-email", "");
    let key = dialog::entry("signing-key", "");
    dialog::grid_row(&g, 0, "Name:", &name);
    dialog::grid_row(&g, 1, "Email:", &email);
    dialog::grid_row(&g, 2, "Signing key ID:", &key);
    page.pack_start(&dialog::frame("User Info", &g), false, false, 0);
    if let Some(v) = git::version() {
        page.pack_start(&dialog::label(&format!("git version {v}")), false, false, 0);
    }
    stack.add_titled(&page, "git", "Git");
    let cwd = repo.as_ref().map(|r| r.root.clone()).unwrap_or_else(glib::home_dir);
    let load = {
        let (name, email, key, scope, cwd) = (name.clone(), email.clone(), key.clone(), scope.clone(), cwd.clone());
        Rc::new(move || {
            let s = if scope[1].is_active() { "--local" } else { "--global" };
            let get = |k: &str| git::run(&cwd, &["config", s, "--get", k]).unwrap_or_default().trim().to_string();
            name.set_text(&get("user.name"));
            email.set_text(&get("user.email"));
            key.set_text(&get("user.signingkey"));
        })
    };
    for r in &scope {
        let l = load.clone();
        r.connect_toggled(move |r| {
            if r.is_active() {
                l();
            }
        });
    }
    load();

    // Icon Overlays
    let page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.set_border_width(8);
    let overlays = dialog::check("Show overlay icons on files and folders in working trees", "show-overlays", w.app.settings.borrow().vcs_overlays);
    page.pack_start(&overlays, false, false, 0);
    let legend = gtk::Grid::new();
    legend.set_row_spacing(4);
    legend.set_column_spacing(8);
    for (i, s) in [Status::Normal, Status::Modified, Status::Added, Status::Deleted, Status::Conflicted, Status::Unversioned, Status::Ignored].iter().enumerate() {
        if let Some(pb) = super::icons::overlay(*s, 16) {
            legend.attach(&gtk::Image::from_pixbuf(Some(&pb)), 0, i as i32, 1, 1);
        }
        legend.attach(&dialog::label(s.label()), 1, i as i32, 1, 1);
    }
    page.pack_start(&dialog::frame("Overlays", &legend), false, false, 0);
    stack.add_titled(&page, "overlays", "Icon Overlays");

    d.add_button("OK", gtk::ResponseType::Ok);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    d.add_button("Apply", gtk::ResponseType::Apply);
    let ww = Rc::downgrade(&w.me());
    d.connect_response(move |d, r| {
        if matches!(r, gtk::ResponseType::Ok | gtk::ResponseType::Apply) {
            let Some(w) = ww.upgrade() else { return };
            let s = if scope[1].is_active() { "--local" } else { "--global" };
            for (k, e) in [("user.name", &name), ("user.email", &email), ("user.signingkey", &key)] {
                let v = e.text().trim().to_string();
                let res = if v.is_empty() { git::run(&cwd, &["config", s, "--unset", k]).or(Ok::<_, String>(String::new())) } else { git::run(&cwd, &["config", s, k, &v]) };
                if let Err(e) = res {
                    dialog::error(&w.win, &e);
                }
            }
            let on = overlays.is_active();
            if w.app.settings.borrow().vcs_overlays != on {
                w.app.settings.borrow_mut().vcs_overlays = on;
                w.app.save_settings_soon();
                w.app.for_each_window(|win| {
                    for p in win.panes() {
                        super::overlay::update(win, &p);
                    }
                });
            }
        }
        if r != gtk::ResponseType::Apply {
            unsafe { d.destroy() };
        }
    });
    d.show_all();
}

pub fn about(w: &Window) {
    let d = dialog::new(w, "About", "about");
    let area = d.content_area();
    area.set_border_width(16);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.pack_start(&super::icons::image("tgit", 64), false, false, 0);
    let text = gtk::Label::new(None);
    text.set_xalign(0.0);
    text.set_markup(&format!(
        "<b>Git integration of failBrauwser {}</b>\n\nOverlay icons, menus and dialogs in the style of TortoiseGit,\nworking through the git command.\n\ngit version {}",
        env!("CARGO_PKG_VERSION"),
        glib::markup_escape_text(&git::version().unwrap_or_else(|| "not found".into()))
    ));
    row.pack_start(&text, true, true, 0);
    area.pack_start(&row, true, true, 0);
    d.add_button("OK", gtk::ResponseType::Ok);
    d.connect_response(|d, _| unsafe { d.destroy() });
    d.show_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(path: &str, status: Status) -> Change {
        Change { path: path.into(), old_path: None, status, staged: false, added: None, removed: None }
    }

    #[test]
    fn remote_commands() {
        assert_eq!(pull_cmd("origin", "main", false, false, true, false, true, false).display(), "git pull --progress -v --no-rebase --no-ff --prune origin main");
        assert_eq!(push_cmd(Some("origin"), "main", "main", false, None, false, true).display(), "git push --progress -v -u origin main");
        assert_eq!(push_cmd(Some("up"), "topic", "feature", false, Some(false), true, false).display(), "git push --progress -v --force-with-lease --tags up topic:feature");
        assert_eq!(push_cmd(None, "main", "", true, Some(true), false, false).display(), "git push --progress -v --force --all origin");
        assert_eq!(clone_cmd("https://x/r.git", "/tmp/r", Some(1), true, Some("dev"), false, false).display(), "git clone --progress -v --depth 1 --recursive --branch dev https://x/r.git /tmp/r");
    }

    #[test]
    fn switching_to_a_remote_branch_tracks_it() {
        let remotes = vec!["origin/feature".to_string()];
        assert_eq!(switch_cmd("origin/feature", &remotes, None, false, false, false).display(), "git checkout -b feature --track origin/feature");
        assert_eq!(switch_cmd("main", &remotes, None, true, false, false).display(), "git checkout -f main");
        assert_eq!(switch_cmd("v1", &remotes, Some("fix"), false, true, true).display(), "git checkout -B fix --track v1");
    }

    #[test]
    fn reverting_added_and_changed_files() {
        let mut renamed = ch("new.txt", Status::Renamed);
        renamed.old_path = Some("old.txt".into());
        let cmds = revert_cmds(true, &[ch("a", Status::Added), ch("m", Status::Modified), ch("d", Status::Deleted), renamed, ch("u", Status::Unversioned)]);
        let shown: Vec<String> = cmds.iter().map(Cmd::display).collect();
        assert_eq!(shown, ["git rm --cached -r -q -f -- a new.txt", "git checkout HEAD -- m d old.txt"]);
        assert_eq!(revert_cmds(false, &[ch("m", Status::Modified)])[0].display(), "git rm --cached -r -q -f -- m");
    }

    #[test]
    fn clean_up_kinds() {
        assert_eq!(cleanup_cmd(0, true, false, &[String::new()]).display(), "git clean -f -d -- .");
        assert_eq!(cleanup_cmd(1, false, true, &["build".into()]).display(), "git clean -f -X -n -- build");
        assert_eq!(cleanup_cmd(2, true, false, &["a".into()]).display(), "git clean -f -d -x -- a");
    }

    #[test]
    fn ignore_lines_like_tortoisegit() {
        assert_eq!(ignore_line("src/ui", "*.log", false, 0), (".gitignore".into(), "/src/ui/*.log".into()));
        assert_eq!(ignore_line("src/ui", "*.log", true, 0), (".gitignore".into(), "/src/ui/**/*.log".into()));
        assert_eq!(ignore_line("", "build", true, 0), (".gitignore".into(), "build".into()));
        assert_eq!(ignore_line("", "build", false, 0), (".gitignore".into(), "/build".into()));
        assert_eq!(ignore_line("src", "x.tmp", false, 1), ("src/.gitignore".into(), "/x.tmp".into()));
        assert_eq!(ignore_line("src", "x.tmp", true, 1), ("src/.gitignore".into(), "x.tmp".into()));
        assert_eq!(ignore_line("src", "x.tmp", false, 2).0, "\0exclude");
    }
}
