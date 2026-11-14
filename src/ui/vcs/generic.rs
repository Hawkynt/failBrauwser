//! The Repository items of the tools other than Git, in TortoiseGit's windows: the same file
//! lists, questions and progress window, with the commands of each tool's own client taken from
//! `vcs::ops`. Git keeps TortoiseGit's own dialogs (`dialogs`), with their Git-only options.

use super::super::window::Window;
use super::dialog;
use super::filelist::FileList;
use super::progress;
use super::{Target, Wc};
use failbrauwser::vcs::ops::{self, Op};
use failbrauwser::vcs::state::{Change, Status};
use failbrauwser::vcs::Tool;
use gtk::prelude::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Runs an item's client calls in the progress window, from the top of the working copy; the
/// overlays and the folder follow when it is done.
pub fn run(w: &Window, wc: &Wc, ops_: &[Op]) {
    let tool = wc.tool();
    let cmds: Vec<progress::Cmd> = ops_.iter().flat_map(|op| progress::tool_cmds(tool, ops::runs(tool, op))).collect();
    if cmds.is_empty() {
        return;
    }
    progress::run(w, wc.root(), wc.root(), cmds, Vec::new(), |w, _| {
        for p in w.panes() {
            p.reload();
        }
        super::overlay::refresh_all(w);
    });
}

/// Runs `ok` with the dialog's answer when it is answered with OK.
fn on_ok(d: &gtk::Dialog, f: impl Fn() -> bool + 'static) {
    d.connect_response(move |d, r| {
        if r == gtk::ResponseType::Ok && !f() {
            return;
        }
        unsafe { d.destroy() };
    });
}

/// One question of one line: a dialog titled after the item, a field (with suggestions to pick
/// from where there are some) and OK. An empty answer runs nothing.
pub fn ask(w: &Window, path: &Path, what: &str, icon: &str, prompt: &str, initial: &str, suggestions: &[String], ok: impl Fn(&Window, String) + 'static) {
    let d = dialog::new(w, &dialog::title(path, what), icon);
    d.set_default_size(480, -1);
    let area = d.content_area();
    let g = dialog::grid();
    let field: gtk::Entry = if suggestions.is_empty() {
        dialog::entry("answer", initial)
    } else {
        let c = gtk::ComboBoxText::with_entry();
        for s in suggestions {
            c.append_text(s);
        }
        let e = c.child().and_then(|c| c.downcast::<gtk::Entry>().ok()).expect("a combo with an entry has one");
        e.set_widget_name("answer");
        e.set_text(initial);
        dialog::grid_row(&g, 1, "", &c);
        e
    };
    field.set_width_chars(40);
    field.set_activates_default(true);
    if suggestions.is_empty() {
        dialog::grid_row(&g, 0, prompt, &field);
    } else {
        g.attach(&dialog::label(prompt), 0, 0, 2, 1);
    }
    area.pack_start(&g, false, false, 0);
    dialog::ok_cancel(&d);
    d.set_default_response(gtk::ResponseType::Ok);
    let ww = Rc::downgrade(&w.me());
    let f2 = field.clone();
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let answer = f2.text().trim().to_string();
        if !answer.is_empty() {
            ok(&w, answer);
        }
        true
    });
    d.show_all();
    field.grab_focus();
}

/// A list of files to tick (Revert, Add, Resolve, Lock): OK runs `ok` with the ticked ones.
pub fn pick_files(w: &Window, title: &str, icon: &str, intro: &str, wc: &Wc, changes: Vec<Change>, ok: impl Fn(&Window, Vec<String>) + 'static) {
    let d = dialog::new(w, title, icon);
    d.set_default_size(640, 420);
    let area = d.content_area();
    area.pack_start(&dialog::label(intro), false, false, 0);
    let list = FileList::new("files", true);
    area.pack_start(&list.scroll, true, true, 0);
    let all = dialog::check("Select / deselect all", "select-all", true);
    area.pack_start(&all, false, false, 0);
    let lw = Rc::downgrade(&list);
    all.connect_toggled(move |c| {
        if let Some(l) = lw.upgrade() {
            let on = c.is_active();
            l.check_where(|_| on);
        }
    });
    list.set(changes, |_| true);
    let (ww, wc2) = (Rc::downgrade(&w.me()), wc.clone());
    list.connect_activated(move |c| {
        if let Some(w) = ww.upgrade() {
            if c.status.is_change() {
                super::diff::working(&w, &wc2, &c.path);
            }
        }
    });
    dialog::ok_cancel(&d);
    let (ww, lw) = (Rc::downgrade(&w.me()), Rc::downgrade(&list));
    on_ok(&d, move || {
        let (Some(w), Some(l)) = (ww.upgrade(), lw.upgrade()) else { return true };
        let files: Vec<String> = l.checked().into_iter().map(|c| c.path).collect();
        if !files.is_empty() {
            ok(&w, files);
        }
        true
    });
    d.show_all();
}

/// The changes under the target that `keep` lets through, read on a worker thread, then `then`.
fn with_changes(w: &Window, t: &Target, wc: &Wc, unversioned: bool, keep: impl Fn(&Change) -> bool + Send + 'static, then: impl FnOnce(&Window, Vec<Change>) + 'static) {
    let (wc2, paths, ww) = (wc.clone(), t.rel(), Rc::downgrade(&w.me()));
    super::background(move || wc2.changes(&paths, unversioned, false).unwrap_or_default().into_iter().filter(|c| keep(c)).collect::<Vec<_>>(), move |changes| {
        if let Some(w) = ww.upgrade() {
            then(&w, changes);
        }
    });
}

pub fn update(w: &Window, _t: Target, wc: Wc) {
    run(w, &wc, &[Op::Update]);
}

/// Mercurial's Sync: what the others have, merged in, then what is ours, sent out.
pub fn sync(w: &Window, _t: Target, wc: Wc) {
    run(w, &wc, &[Op::Pull, Op::Push]);
}

pub fn pull(w: &Window, _t: Target, wc: Wc) {
    run(w, &wc, &[Op::Pull]);
}

pub fn fetch(w: &Window, _t: Target, wc: Wc) {
    run(w, &wc, &[Op::Fetch]);
}

pub fn push(w: &Window, _t: Target, wc: Wc) {
    run(w, &wc, &[Op::Push]);
}

pub fn revert(w: &Window, t: Target, wc: Wc) {
    let title = dialog::title(&t.title_path(), "Revert");
    let wc2 = wc.clone();
    with_changes(w, &t, &wc, false, |c| c.status.is_change(), move |w, changes| {
        let wc3 = wc2.clone();
        pick_files(w, &title, "revert", "Revert", &wc2, changes, move |w, files| run(w, &wc3, &[Op::Revert { paths: files }]));
    });
}

/// Reverts these files after asking (from a file list's right-click menu).
pub fn revert_files(w: &Window, wc: &Wc, files: &[Change]) {
    let (ww, wc, paths) = (Rc::downgrade(&w.me()), wc.clone(), files.iter().map(|c| c.path.clone()).collect::<Vec<_>>());
    let what = if paths.len() == 1 { paths[0].clone() } else { format!("{} files", paths.len()) };
    dialog::confirm(&w.win, &format!("Do you really want to revert {what}?"), "The changes are lost.", "Revert", move || {
        if let Some(w) = ww.upgrade() {
            run(&w, &wc, &[Op::Revert { paths: paths.clone() }]);
        }
    });
}

pub fn add(w: &Window, t: Target, wc: Wc) {
    let title = dialog::title(&t.title_path(), "Add");
    let wc2 = wc.clone();
    with_changes(w, &t, &wc, true, |c| c.status == Status::Unversioned, move |w, changes| {
        let wc3 = wc2.clone();
        pick_files(w, &title, "add", "Add", &wc2, changes, move |w, files| run(w, &wc3, &[Op::Add { paths: files }]));
    });
}

pub fn resolve(w: &Window, t: Target, wc: Wc) {
    let title = dialog::title(&t.title_path(), "Resolve");
    let wc2 = wc.clone();
    with_changes(w, &t, &wc, false, |c| c.status == Status::Conflicted, move |w, changes| {
        let wc3 = wc2.clone();
        pick_files(w, &title, "resolve", "Mark these conflicts as resolved:", &wc2, changes, move |w, files| run(w, &wc3, &[Op::Resolve { paths: files }]));
    });
}

pub fn delete(w: &Window, t: Target, wc: Wc, keep: bool) {
    let rel = t.rel();
    let what = match t.paths.as_slice() {
        [one] => one.display().to_string(),
        many => format!("{} items", many.len()),
    };
    let ww = Rc::downgrade(&w.me());
    let text = if keep { format!("Remove {what} from the repository and keep it on disk?") } else { format!("Do you really want to delete {what}?") };
    dialog::confirm(&w.win, &text, "", "Delete", move || {
        let Some(w) = ww.upgrade() else { return };
        // CVS takes a file out of the repository only once it is gone from the folder: the copy
        // is set aside and put back for "keep local".
        let aside: Vec<(PathBuf, Vec<u8>)> = if keep && wc.tool() == Tool::Cvs {
            rel.iter().filter_map(|r| std::fs::read(wc.root().join(r)).ok().map(|b| (wc.root().join(r), b))).collect()
        } else {
            Vec::new()
        };
        for (p, _) in &aside {
            let _ = std::fs::remove_file(p);
        }
        run(&w, &wc, &[Op::Delete { paths: rel.clone(), keep_local: keep }]);
        for (p, b) in aside {
            let _ = std::fs::write(p, b);
        }
    });
}

pub fn rename(w: &Window, t: Target, wc: Wc) {
    let Some(path) = t.paths.first().cloned() else { return };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (shown, initial) = (path.clone(), name.clone());
    ask(w, &shown, "Rename", "rename", "New name:", &initial, &[], move |w, new| {
        if new == name || new.contains('/') {
            return;
        }
        let (Some(from), Some(to)) = (wc.relative(&path), path.parent().and_then(|p| wc.relative(&p.join(&new)))) else { return };
        run(w, &wc, &[Op::Rename { from, to }]);
    });
}

/// Adds a name to the tool's ignore record: a line of its ignore file in the item's folder, or
/// Subversion's `svn:ignore` property of that folder.
pub fn ignore(w: &Window, t: Target, wc: Wc, pattern: String) {
    for p in &t.paths {
        let (Some(dir), Some(name)) = (p.parent(), p.file_name()) else { continue };
        let pat = if pattern.is_empty() { name.to_string_lossy().into_owned() } else { pattern.clone() };
        let result = match ops::ignore_file(wc.tool()) {
            // Mercurial's .hgignore lives at the top and reads regular expressions unless told.
            Some(".hgignore") => failbrauwser::vcs::git::add_ignore(&wc.root().join(".hgignore"), &format!("glob:{}", wc.relative(p).unwrap_or(pat.clone()))).map_err(|e| e.to_string()),
            Some(file) => failbrauwser::vcs::git::add_ignore(&dir.join(file), &pat).map_err(|e| e.to_string()),
            None if wc.tool() == Tool::Subversion => {
                let rel = wc.relative(dir).unwrap_or_default();
                let at = if rel.is_empty() { ".".to_string() } else { rel };
                let current = failbrauwser::vcs::run::text("svn", wc.root(), &["propget", "svn:ignore", "--", &at]).unwrap_or_default();
                let value = ops::svn_ignore_value(&current, &pat);
                let a = failbrauwser::vcs::run::call(Tool::Subversion, wc.root(), &["propset", "svn:ignore", &value, "--", &at]);
                if a.ok { Ok(()) } else { Err(a.text) }
            }
            None => Ok(()),
        };
        if let Err(e) = result {
            dialog::error(&w.win, &e);
        }
    }
    for p in w.panes() {
        p.reload();
    }
    super::overlay::refresh_all(w);
}

pub fn cleanup(w: &Window, t: Target, wc: Wc) {
    let ww = Rc::downgrade(&w.me());
    let what = if wc.tool() == Tool::Subversion { "Clean up the working copy and remove the files it does not know?" } else { "Remove the files the repository does not know?" };
    let _ = t.title_path();
    dialog::confirm(&w.win, what, "Unversioned files are deleted for good.", "Clean up", move || {
        if let Some(w) = ww.upgrade() {
            run(&w, &wc, &[Op::Cleanup]);
        }
    });
}

pub fn lock(w: &Window, t: Target, wc: Wc) {
    let paths = t.rel();
    ask(w, &t.title_path(), "Lock", "lock", "Message:", "", &[], move |w, message| run(w, &wc, &[Op::Lock { paths: paths.clone(), message }]));
}

pub fn unlock(w: &Window, t: Target, wc: Wc) {
    run(w, &wc, &[Op::Unlock { paths: t.rel() }]);
}

/// What a tool switches to: Subversion a place in its repository, the others a branch, tag or
/// revision.
pub fn switch(w: &Window, t: Target, wc: Wc) {
    let refs = wc.refs();
    let (prompt, mut suggestions): (&str, Vec<String>) = match wc.tool() {
        Tool::Subversion => ("Switch to (^/branches/name, ^/tags/name, ^/trunk):", vec!["^/trunk".into()]),
        _ => ("Switch to branch, tag or revision:", refs.branches.clone()),
    };
    suggestions.extend(refs.tags.iter().cloned());
    ask(w, &t.title_path(), "Switch/Checkout", "switch", prompt, "", &suggestions, move |w, to| run(w, &wc, &[Op::Switch { to }]));
}

pub fn merge(w: &Window, t: Target, wc: Wc) {
    let refs = wc.refs();
    let prompt = match wc.tool() {
        Tool::Subversion => "Merge from (^/branches/name):",
        Tool::Cvs => "Merge from branch or tag:",
        _ => "Merge from branch or revision:",
    };
    ask(w, &t.title_path(), "Merge", "merge", prompt, "", &refs.branches, move |w, from| run(w, &wc, &[Op::Merge { from }]));
}

pub fn branch(w: &Window, t: Target, wc: Wc) {
    ask(w, &t.title_path(), "Create Branch", "branch", "Branch:", "", &[], move |w, name| {
        let message = format!("Create branch {name}");
        run(w, &wc, &[Op::Branch { name, message }]);
    });
}

pub fn tag(w: &Window, t: Target, wc: Wc) {
    ask(w, &t.title_path(), "Create Tag", "tag", "Tag:", "", &[], move |w, name| {
        let message = format!("Create tag {name}");
        run(w, &wc, &[Op::Tag { name, message }]);
    });
}

pub fn export(w: &Window, t: Target, wc: Wc) {
    let suggestion = t.folder.parent().unwrap_or(&t.folder).join(format!("{}-export", wc.root().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    ask(w, &t.title_path(), "Export", "export", "Export to folder:", &suggestion.to_string_lossy(), &[], move |w, dest| run(w, &wc, &[Op::Export { dest }]));
}

pub fn stash_save(w: &Window, t: Target, wc: Wc) {
    ask(w, &t.title_path(), "Stash", "stashsave", "Message (optional):", "", &[], move |w, message| run(w, &wc, &[Op::StashSave { message }]));
}

pub fn stash_pop(w: &Window, _t: Target, wc: Wc) {
    run(w, &wc, &[Op::StashPop]);
}

/// A patch of the working copy's changes: Mercurial exports its last commit; Subversion and CVS
/// write their diff, which is saved as the file named.
pub fn patch_create(w: &Window, t: Target, wc: Wc) {
    let suggestion = t.folder.join(format!("{}.patch", wc.root().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "changes".into())));
    ask(w, &t.title_path(), "Create Patch Serial", "patchcreate", "Save the patch as:", &suggestion.to_string_lossy(), &[], move |w, dest| {
        let tool = wc.tool();
        if tool == Tool::Mercurial {
            run(w, &wc, &[Op::PatchCreate { dest }]);
            return;
        }
        let runs = ops::runs(tool, &Op::PatchCreate { dest: dest.clone() });
        let Some(argv) = runs.first() else { return };
        // A diff that finds differences ends with 1 for CVS: its words are the patch either way.
        let out = std::process::Command::new(tool.cli()).current_dir(wc.root()).args(argv).env("LC_ALL", "C").output();
        match out {
            Ok(o) if o.status.code().is_some_and(|c| c <= 1) => {
                if let Err(e) = std::fs::write(&dest, &o.stdout) {
                    dialog::error(&w.win, &format!("{dest}: {e}"));
                }
            }
            Ok(o) => dialog::error(&w.win, String::from_utf8_lossy(&o.stderr).trim()),
            Err(e) => dialog::error(&w.win, &format!("{} could not be started: {e}", tool.cli())),
        }
        for p in w.panes() {
            p.reload();
        }
    });
}

pub fn patch_apply(w: &Window, t: Target, wc: Wc) {
    let c = gtk::FileChooserDialog::with_buttons(Some("Apply Patch Serial"), Some(&w.win), gtk::FileChooserAction::Open, &[("Cancel", gtk::ResponseType::Cancel), ("Apply", gtk::ResponseType::Accept)]);
    let _ = c.set_current_folder(&t.folder);
    let f = gtk::FileFilter::new();
    f.set_name(Some("Patch files"));
    f.add_pattern("*.patch");
    f.add_pattern("*.diff");
    c.add_filter(f);
    let ww = Rc::downgrade(&w.me());
    c.connect_response(move |c, r| {
        let file = c.filename();
        unsafe { c.destroy() };
        let (Some(w), Some(file)) = (ww.upgrade(), file) else { return };
        if r == gtk::ResponseType::Accept {
            run(&w, &wc, &[Op::PatchApply { file: file.to_string_lossy().into_owned() }]);
        }
    });
    c.show();
}

/// The tools that can make a working copy in a folder no tool owns.
fn tools_with(cap: failbrauwser::vcs::caps::Cap) -> Vec<Tool> {
    Tool::all().iter().copied().filter(|t| t.has(cap)).collect()
}

/// Clone…: no marker says which tool should clone into a folder no tool owns, so the address,
/// the folder it makes and the tool are asked together; Git's own options (TortoiseGit's clone
/// dialog's) show while Git is the tool chosen.
pub fn clone(w: &Window, path: &Path) {
    let d = dialog::new(w, &dialog::title(path, "Clone"), "clone");
    d.set_default_size(560, -1);
    let area = d.content_area();
    let g = dialog::grid();
    let tools = tools_with(failbrauwser::vcs::caps::Cap::Clone);
    let names: Vec<String> = tools.iter().map(|t| t.label().to_string()).collect();
    let tool = dialog::combo("tool", &names, "Git");
    let clip = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD).wait_for_text().map(|s| s.to_string()).filter(|s| s.contains("://") || s.starts_with("git@"));
    let url = dialog::entry("url", clip.as_deref().unwrap_or(""));
    url.set_placeholder_text(Some("Address (CVS: the CVSROOT, a space, the module)"));
    let dir = dialog::entry("directory", &path.join(clip.as_deref().map(failbrauwser::vcs::git::clone_dir_name).unwrap_or_default()).to_string_lossy());
    dialog::grid_row(&g, 0, "Tool:", &tool);
    dialog::grid_row(&g, 1, "URL:", &url);
    dialog::grid_row(&g, 2, "Directory:", &dir);
    area.pack_start(&g, false, false, 0);
    let (d2, base) = (dir.clone(), path.to_path_buf());
    url.connect_changed(move |u| {
        let text = u.text().to_string();
        let last = text.rsplit(|c| c == ' ' || c == '/').find(|s| !s.is_empty()).unwrap_or("");
        d2.set_text(&base.join(failbrauwser::vcs::git::clone_dir_name(last)).to_string_lossy());
    });
    // Git's options.
    let og = dialog::grid();
    let depth = dialog::check("Depth", "use-depth", false);
    let depth_n = gtk::SpinButton::with_range(1.0, 100000.0, 1.0);
    depth_n.set_widget_name("depth");
    let recursive = dialog::check("Recursive", "recursive", false);
    let branch = dialog::check("Branch", "use-branch", false);
    let branch_name = dialog::entry("branch", "");
    let no_checkout = dialog::check("No Checkout", "no-checkout", false);
    let bare = dialog::check("Bare", "bare", false);
    og.attach(&depth, 0, 0, 1, 1);
    og.attach(&depth_n, 1, 0, 1, 1);
    og.attach(&recursive, 2, 0, 1, 1);
    og.attach(&branch, 0, 1, 1, 1);
    og.attach(&branch_name, 1, 1, 1, 1);
    og.attach(&no_checkout, 2, 1, 1, 1);
    og.attach(&bare, 2, 2, 1, 1);
    og.set_column_spacing(12);
    og.set_row_spacing(6);
    let options = dialog::frame("Options", &og);
    area.pack_start(&options, false, false, 0);
    let o2 = options.clone();
    tool.connect_changed(move |c| o2.set_visible(dialog::combo_text(c) == "Git"));
    dialog::ok_cancel(&d);
    let (ww, base) = (Rc::downgrade(&w.me()), path.to_path_buf());
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        let (u, target) = (url.text().trim().to_string(), dir.text().trim().to_string());
        let chosen = dialog::combo_text(&tool);
        let Some(t) = tools.iter().copied().find(|t| t.label() == chosen) else { return true };
        if u.is_empty() || target.is_empty() {
            dialog::error(&w.win, "Enter the URL to clone and the directory to clone into.");
            return false;
        }
        let cmds = if t == Tool::Git {
            vec![super::dialogs::clone_cmd(&u, &target, depth.is_active().then(|| depth_n.value_as_int()), recursive.is_active(), branch.is_active().then(|| branch_name.text().to_string()).as_deref(), no_checkout.is_active(), bare.is_active())]
        } else {
            progress::tool_cmds(t, ops::runs(t, &Op::Clone { url: u, dir: target }))
        };
        if cmds.is_empty() {
            dialog::error(&w.win, "A CVS address is the CVSROOT, a space and the module.");
            return false;
        }
        progress::run(&w, &base, &base, cmds, Vec::new(), |w, _| {
            for p in w.panes() {
                p.reload();
            }
            super::overlay::refresh_all(w);
        });
        true
    });
    d.show_all();
}

/// Create repository here…: the tool asked for; Git can make a bare repository.
pub fn init(w: &Window, path: &Path) {
    let tools = tools_with(failbrauwser::vcs::caps::Cap::Init);
    let names: Vec<String> = tools.iter().map(|t| t.label().to_string()).collect();
    let d = dialog::new(w, &dialog::title(path, "Create repository"), "createrepo");
    let area = d.content_area();
    let g = dialog::grid();
    let tool = dialog::combo("tool", &names, "Git");
    dialog::grid_row(&g, 0, "Tool:", &tool);
    area.pack_start(&g, false, false, 0);
    let bare = dialog::check("Make it Bare (No working directory)", "bare", false);
    area.pack_start(&bare, false, false, 0);
    let b2 = bare.clone();
    tool.connect_changed(move |c| b2.set_sensitive(dialog::combo_text(c) == "Git"));
    dialog::ok_cancel(&d);
    let (ww, path) = (Rc::downgrade(&w.me()), path.to_path_buf());
    on_ok(&d, move || {
        let Some(w) = ww.upgrade() else { return true };
        match tools.iter().copied().find(|t| t.label() == dialog::combo_text(&tool)) {
            Some(Tool::Git) => super::dialogs::init_here(&w, &path, bare.is_active()),
            Some(t) => {
                progress::run(&w, &path, &path, progress::tool_cmds(t, ops::runs(t, &Op::Init { dir: ".".into() })), Vec::new(), |w, _| {
                    for p in w.panes() {
                        p.reload();
                    }
                    super::overlay::refresh_all(w);
                });
            }
            None => {}
        }
        true
    });
    d.show_all();
}
