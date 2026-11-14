//! Version control in the file manager, the way TortoiseGit does it in Explorer, for every tool
//! (Git, Mercurial, Subversion, CVS, Visual Source Safe): overlay icons on the files and folders
//! of working copies, "⟨Tool⟩ Sync…" or "⟨Tool⟩ Update", "⟨Tool⟩ Commit -> "branch"…" and the
//! Repository submenu in the right-click menu, and TortoiseGit's dialogs for what they do. What a
//! tool has no equivalent for (`vcs::caps`) is left out of the menu.

pub mod askpass;
pub mod blame;
pub mod commit;
pub mod dialog;
pub mod dialogs;
pub mod diff;
pub mod filelist;
pub mod generic;
pub mod icons;
pub mod log;
pub mod overlay;
pub mod progress;
pub mod sync;
pub mod testing;

use super::model::Item;
use super::pane::Pane;
use super::window::Window;
use failbrauwser::vcs::backend::Backend;
use failbrauwser::vcs::backend;
use failbrauwser::vcs::caps::Cap;
use failbrauwser::vcs::git::{self, Repo, Status};
use failbrauwser::vcs::Tool;
use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::path::PathBuf;
use std::rc::Rc;

/// A working copy as the windows read it, whatever tool owns it.
pub type Wc = std::sync::Arc<dyn Backend>;

/// A Git repository as a working copy of the shared windows.
pub fn wc(repo: &Repo) -> Wc {
    std::sync::Arc::new(repo.clone())
}

/// What a version-control menu item works on: the selected files and folders, or the folder
/// shown, and the working copy holding them.
#[derive(Clone, Debug)]
pub struct Target {
    pub wc: Option<Wc>,
    /// The Git repository, where the working copy is Git's: for TortoiseGit's own dialogs.
    pub repo: Option<Repo>,
    pub paths: Vec<PathBuf>,
    /// Each path's overlay status, where known.
    pub status: Vec<Option<Status>>,
    pub is_dir: Vec<bool>,
    /// The folder shown.
    pub folder: PathBuf,
    /// Nothing is selected: the menu of the folder itself.
    pub background: bool,
}

impl Target {
    /// The paths relative to the top of the working copy ("" for all of it).
    pub fn rel(&self) -> Vec<String> {
        let Some(wc) = &self.wc else { return Vec::new() };
        let rel: Vec<String> = self.paths.iter().filter_map(|p| wc.relative(p)).collect();
        if rel.iter().any(String::is_empty) { vec![String::new()] } else { rel }
    }

    /// What the dialog titles name: the one path, or the folder.
    pub fn title_path(&self) -> PathBuf {
        match self.paths.as_slice() {
            [one] => one.clone(),
            _ => self.folder.clone(),
        }
    }

    /// The one file selected (not a folder), relative to the top of the working copy.
    pub fn single_file(&self) -> Option<String> {
        match (self.paths.as_slice(), self.is_dir.as_slice()) {
            ([p], [false]) => self.wc.as_ref()?.relative(p),
            _ => None,
        }
    }

    fn any(&self, f: impl Fn(Option<Status>) -> bool) -> bool {
        self.status.iter().any(|s| f(*s))
    }

    /// The whole working tree of a Git repository.
    pub fn of_repo(repo: &Repo) -> Target {
        Target { wc: Some(wc(repo)), repo: Some(repo.clone()), paths: vec![repo.root.clone()], status: vec![None], is_dir: vec![true], folder: repo.root.clone(), background: true }
    }
}

/// Runs `work` on a worker thread and hands its result to `done` on the UI thread.
pub fn background<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(T) + 'static) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::Builder::new()
        .name("fb-git".into())
        .spawn(move || {
            let _ = tx.send_blocking(work());
        })
        .expect("spawn git worker");
    glib::spawn_future_local(async move {
        if let Ok(v) = rx.recv().await {
            done(v);
        }
    });
}

/// What the version-control items work on in the window's current tab.
pub fn target(w: &Window) -> Option<Target> {
    target_of(&w.current_pane(), &w.current_pane().selected_items())
}

fn target_of(pane: &Rc<Pane>, items: &[Item]) -> Option<Target> {
    let folder = match pane.location() {
        Location::Dir(d) => d,
        Location::Search(s) => s.root.clone(),
        _ => return None,
    };
    if failbrauwser::ops::gvfs::is_gvfs_path(&folder) {
        return None;
    }
    let mut t = Target { wc: None, repo: None, paths: Vec::new(), status: Vec::new(), is_dir: Vec::new(), folder: folder.clone(), background: items.is_empty() };
    if items.is_empty() {
        t.paths.push(folder.clone());
        t.status.push(None);
        t.is_dir.push(true);
    }
    for i in items {
        let (Item::Fs(e) | Item::Found(e, _)) = i else { return None };
        t.paths.push(e.path.clone());
        t.status.push(pane.git_status(&i.os_name().to_string_lossy()).and_then(icons::status_from_code));
        t.is_dir.push(e.is_dir_like());
    }
    // A selected folder that is a working copy of its own is asked about as itself; anything
    // else belongs to the working copy holding the folder shown.
    t.wc = backend::discover(&t.paths[0]);
    // Items of different working copies: none of them.
    if let Some(wc) = &t.wc {
        let same = |p: &PathBuf| backend::discover(p).is_some_and(|o| o.tool() == wc.tool() && o.root() == wc.root());
        if !t.paths.iter().all(same) {
            t.wc = None;
        }
    }
    t.repo = t.wc.as_ref().and_then(|w| w.as_git().cloned());
    Some(t)
}

/// An item of the menu with its icon.
fn item(label: &str, action: &str, icon: &str) -> gio::MenuItem {
    let it = gio::MenuItem::new(Some(label), Some(action));
    if let Some(i) = icons::gicon(icon) {
        it.set_icon(&i);
    }
    it
}

fn item_with(label: &str, action: &str, value: &str, icon: &str) -> gio::MenuItem {
    let it = gio::MenuItem::new(Some(label), None);
    it.set_action_and_target_value(Some(action), Some(&value.to_variant()));
    if let Some(i) = icons::gicon(icon) {
        it.set_icon(&i);
    }
    it
}

fn section(items: Vec<gio::MenuItem>) -> Option<gio::Menu> {
    if items.is_empty() {
        return None;
    }
    let s = gio::Menu::new();
    for i in items {
        s.append_item(&i);
    }
    Some(s)
}

/// Menu texts take '_' as mnemonic mark: names keep theirs.
fn escape(s: &str) -> String {
    s.replace('_', "__")
}

/// The submenu all the items but the quick ones live in.
fn repository_item(sub: &gio::Menu) -> gio::MenuItem {
    let it = gio::MenuItem::new_submenu(Some("Repository"), sub);
    if let Some(i) = icons::gicon("tgit") {
        it.set_icon(&i);
    }
    it
}

/// The version-control part of the right-click menu, as TortoiseGit has it in Explorer: in a
/// working copy "⟨Tool⟩ Sync…" or "⟨Tool⟩ Update", "⟨Tool⟩ Commit -> "branch"…" and the
/// Repository submenu; elsewhere Repository with Clone… and Create repository here… for folders.
pub fn menu_section(w: &Window, pane: &Rc<Pane>, items: &[Item]) -> Option<gio::Menu> {
    if !w.app.settings.borrow().vcs_integration {
        return None;
    }
    let t = target_of(pane, items)?;
    let top = gio::Menu::new();
    let sub = gio::Menu::new();
    let all_dirs = t.is_dir.iter().all(|d| *d);
    match &t.wc {
        None => {
            if !(all_dirs && t.paths.len() == 1) {
                return None;
            }
            sub.append_item(&item("_Clone...", "win.vcs-clone", "clone"));
            sub.append_item(&item("Create _repository here...", "win.vcs-init", "createrepo"));
        }
        Some(wc) => {
            let tool = wc.tool();
            let short = tool.short();
            if tool.has(Cap::Sync) {
                top.append_item(&item(&format!("{short} _Sync..."), "win.vcs-sync", "sync"));
            }
            if tool.has(Cap::Update) {
                top.append_item(&item(&format!("{short} _Update"), "win.vcs-update", "update"));
            }
            top.append_item(&item(&format!("{short} _Commit -> \"{}\"...", escape(&wc.branch_label())), "win.vcs-commit", "commit"));
            for s in repo_submenu(wc, &t) {
                sub.append_section(None, &s);
            }
        }
    }
    let about = section(vec![item("_Settings", "win.vcs-settings", "settings"), item("_About", "win.vcs-about", "about")]).expect("items");
    sub.append_section(None, &about);
    top.append_item(&repository_item(&sub));
    Some(top)
}

/// Whether a stash waits to be popped: Git's stash reflog, Mercurial's shelved changes.
fn has_stash(wc: &Wc) -> bool {
    match (wc.tool(), wc.as_git()) {
        (_, Some(repo)) => repo.common_dir().join("logs/refs/stash").exists(),
        (Tool::Mercurial, _) => std::fs::read_dir(wc.root().join(".hg/shelved")).is_ok_and(|mut d| d.next().is_some()),
        _ => false,
    }
}

/// Whether a merge with conflicts is under way, from the tool's own marker (cheap: the menu
/// is built on the window's thread).
fn merging(wc: &Wc) -> bool {
    match (wc.tool(), wc.as_git()) {
        (_, Some(repo)) => repo.git_dir.join("MERGE_HEAD").exists(),
        (Tool::Mercurial, _) => wc.root().join(".hg/merge/state").exists(),
        _ => false,
    }
}

/// The sections of the Repository submenu inside a working copy, TortoiseGit's, each item where
/// it applies and where the tool has it.
fn repo_submenu(wc: &Wc, t: &Target) -> Vec<gio::Menu> {
    let tool = wc.tool();
    let has = |c: Cap| tool.has(c);
    let mut out = Vec::new();
    let one_file = t.single_file();
    let any_dir = t.is_dir.iter().any(|d| *d);
    let changed = t.background || any_dir || t.any(|s| s.is_some_and(Status::is_change));
    let versioned = t.any(|s| matches!(s, Some(s) if s != Status::Unversioned && s != Status::Ignored));
    let unversioned = t.any(|s| s == Some(Status::Unversioned));
    let conflicted = t.any(|s| s == Some(Status::Conflicted)) || ((t.background || any_dir) && merging(wc));
    let mut remote = Vec::new();
    if has(Cap::Pull) {
        remote.push(item("_Pull...", "win.vcs-pull", "pull"));
    }
    if has(Cap::Fetch) {
        remote.push(item("_Fetch...", "win.vcs-fetch", "fetch"));
    }
    if has(Cap::Push) {
        remote.push(item("Pu_sh...", "win.vcs-push", "push"));
    }
    out.extend(section(remote));
    let mut diff = Vec::new();
    if one_file.is_some() && t.any(|s| s.is_some_and(Status::is_change)) {
        diff.push(item("_Diff", "win.vcs-diff", "diff"));
    }
    if one_file.is_some() && versioned && !t.any(|s| s == Some(Status::Added)) {
        diff.push(item("Diff with previous _version", "win.vcs-diff-prev", "diffprev"));
    }
    out.extend(section(diff));
    let mut log = vec![item("Show _log", "win.vcs-log", "log")];
    if has(Cap::Reflog) {
        log.push(item("Show _Reflog", "win.vcs-reflog", "reflog"));
    }
    if has(Cap::Blame) && one_file.is_some() && versioned && !t.any(|s| s == Some(Status::Added)) {
        log.push(item("_Blame", "win.vcs-blame", "blame"));
    }
    log.push(item("Check for _modifications", "win.vcs-checkmods", "checkmods"));
    if has(Cap::Stash) {
        log.push(item("Stash Sa_ve", "win.vcs-stash-save", "stashsave"));
        if has_stash(wc) {
            log.push(item("Stash P_op", "win.vcs-stash-pop", "stashpop"));
        }
    }
    out.extend(section(log));
    let mut fix = Vec::new();
    if has(Cap::Resolve) && conflicted {
        fix.push(item("Resol_ve...", "win.vcs-resolve", "resolve"));
    }
    if changed {
        fix.push(item("_Revert...", "win.vcs-revert", "revert"));
    }
    if has(Cap::Cleanup) && (t.background || t.is_dir.iter().all(|d| *d)) {
        fix.push(item("Clean _up...", "win.vcs-cleanup", "cleanup"));
    }
    if has(Cap::Lock) && !t.background && versioned {
        fix.push(item("Get Loc_k...", "win.vcs-lock", "lock"));
        fix.push(item("Release Lock", "win.vcs-unlock", "unlock"));
    }
    out.extend(section(fix));
    let mut refs = vec![item("S_witch/Checkout...", "win.vcs-switch", "switch")];
    if has(Cap::Merge) {
        refs.push(item("_Merge...", "win.vcs-merge", "merge"));
    }
    if has(Cap::Branch) {
        refs.push(item("Create _Branch...", "win.vcs-branch", "branch"));
    }
    refs.push(item("Create _Tag...", "win.vcs-tag", "tag"));
    if has(Cap::Export) {
        refs.push(item("_Export...", "win.vcs-export", "export"));
    }
    out.extend(section(refs));
    let mut files = Vec::new();
    if t.background || unversioned || any_dir {
        files.push(item("_Add...", "win.vcs-add", "add"));
    }
    if !t.background && versioned {
        files.push(item("_Delete", "win.vcs-delete", "delete"));
        if has(Cap::DeleteKeepLocal) {
            files.push(item("Delete (_keep local)", "win.vcs-delete-keep", "deletekeep"));
        }
    }
    if has(Cap::Rename) && !t.background && versioned && t.paths.len() == 1 {
        files.push(item("Re_name...", "win.vcs-rename", "rename"));
    }
    if has(Cap::Ignore) && !t.background && unversioned {
        let ig = gio::Menu::new();
        if let [p] = t.paths.as_slice() {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            for pat in git::ignore_patterns(&name) {
                ig.append_item(&item_with(&escape(&pat), "win.vcs-ignore", &pat, "ignore"));
            }
        } else {
            ig.append_item(&item_with(&format!("{} items by _name", t.paths.len()), "win.vcs-ignore", "", "ignore"));
        }
        let sm = gio::MenuItem::new_submenu(Some("Add to _ignore list"), &ig);
        if let Some(i) = icons::gicon("ignore") {
            sm.set_icon(&i);
        }
        files.push(sm);
    }
    out.extend(section(files));
    let mut patch = Vec::new();
    if has(Cap::PatchCreate) {
        patch.push(item("Create _Patch Serial...", "win.vcs-patch-create", "patchcreate"));
    }
    if has(Cap::PatchApply) {
        patch.push(item("Appl_y Patch Serial...", "win.vcs-patch-apply", "patchapply"));
    }
    out.extend(section(patch));
    out
}

/// Reverts these files after asking: TortoiseGit's way for Git, the tool's own revert elsewhere.
pub fn revert_files(w: &Window, wc: &Wc, files: &[git::Change]) {
    match wc.as_git() {
        Some(repo) => dialogs::revert_files(w, repo, files),
        None => generic::revert_files(w, wc, files),
    }
}

/// Runs `f` with the working copy of the menu's target, or tells why there is none.
fn with_wc(w: &Window, f: impl FnOnce(&Window, Target, Wc)) {
    let Some(t) = target(w) else { return };
    match t.wc.clone() {
        Some(wc) => f(w, t, wc),
        None => dialog::error(&w.win, "This is not a working copy of a version-control tool."),
    }
}

/// An item with a dialog of TortoiseGit's own for Git and the shared one for the other tools.
type Both = (fn(&Window, Target, Repo), fn(&Window, Target, Wc));

fn both(w: &Window, t: Target, wc: Wc, (git_dialog, generic): Both) {
    match t.repo.clone() {
        Some(repo) => git_dialog(w, t, repo),
        None => generic(w, t, wc),
    }
}

pub fn install_actions(w: &Rc<Window>) {
    overlay::window_created(w);
    // The same for every tool: the shared windows read the working copy through its backend.
    let shared: &[(&str, fn(&Window, Target, Wc))] = &[
        ("vcs-commit", |w, t, wc| commit::show(w, &wc, &t.title_path(), t.rel())),
        ("vcs-diff", |w, t, wc| {
            if let Some(f) = t.single_file() {
                diff::working(w, &wc, &f);
            }
        }),
        ("vcs-diff-prev", |w, t, wc| {
            if let Some(f) = t.single_file() {
                diff::previous(w, &wc, &f);
            }
        }),
        ("vcs-log", |w, t, wc| log::show(w, &wc, &t.title_path(), t.rel())),
        ("vcs-blame", |w, t, wc| {
            if let Some(f) = t.single_file() {
                blame::show(w, &wc, &f);
            }
        }),
        ("vcs-checkmods", |w, t, wc| dialogs::check_modifications(w, t, wc)),
        ("vcs-update", generic::update),
        ("vcs-lock", generic::lock),
        ("vcs-unlock", generic::unlock),
    ];
    for (name, f) in shared {
        let f = *f;
        w.add_action(name, move |w| with_wc(w, f));
    }
    // TortoiseGit's own dialog for Git, the shared one elsewhere.
    let per_tool: &[(&str, Both)] = &[
        ("vcs-sync", (sync::show, generic::sync)),
        ("vcs-pull", (dialogs::pull, generic::pull)),
        ("vcs-fetch", (dialogs::fetch, generic::fetch)),
        ("vcs-push", (dialogs::push, generic::push)),
        ("vcs-stash-save", (dialogs::stash_save, generic::stash_save)),
        ("vcs-stash-pop", (dialogs::stash_pop, generic::stash_pop)),
        ("vcs-resolve", (dialogs::resolve, generic::resolve)),
        ("vcs-revert", (dialogs::revert, generic::revert)),
        ("vcs-cleanup", (dialogs::cleanup, generic::cleanup)),
        ("vcs-switch", (dialogs::switch, generic::switch)),
        ("vcs-merge", (dialogs::merge, generic::merge)),
        ("vcs-branch", (dialogs::branch, generic::branch)),
        ("vcs-tag", (dialogs::tag, generic::tag)),
        ("vcs-export", (dialogs::export, generic::export)),
        ("vcs-add", (dialogs::add, generic::add)),
        ("vcs-delete", (|w, t, r| dialogs::delete(w, t, r, false), |w, t, wc| generic::delete(w, t, wc, false))),
        ("vcs-delete-keep", (|w, t, r| dialogs::delete(w, t, r, true), |w, t, wc| generic::delete(w, t, wc, true))),
        ("vcs-rename", (dialogs::rename, generic::rename)),
        ("vcs-patch-create", (dialogs::patch_create, generic::patch_create)),
        ("vcs-patch-apply", (dialogs::patch_apply, generic::patch_apply)),
    ];
    for (name, pair) in per_tool {
        let pair = *pair;
        w.add_action(name, move |w| with_wc(w, |w, t, wc| both(w, t, wc, pair)));
    }
    // Git alone keeps a reflog.
    w.add_action("vcs-reflog", |w| {
        with_wc(w, |w, t, _| {
            if let Some(repo) = t.repo.clone() {
                dialogs::reflog(w, t, repo);
            }
        })
    });
    w.add_action("vcs-clone", |w| {
        if let Some(t) = target(w) {
            generic::clone(w, &t.title_path());
        }
    });
    w.add_action("vcs-init", |w| {
        if let Some(t) = target(w) {
            generic::init(w, &t.title_path());
        }
    });
    w.add_action("vcs-settings", |w| {
        let t = target(w);
        dialogs::settings(w, t.and_then(|t| t.repo));
    });
    w.add_action("vcs-about", dialogs::about);
    let a = gio::SimpleAction::new("vcs-ignore", Some(glib::VariantTy::STRING));
    let weak = Rc::downgrade(w);
    a.connect_activate(move |_, v| {
        let (Some(w), Some(pattern)) = (weak.upgrade(), v.and_then(|v| v.get::<String>())) else { return };
        with_wc(&w, |w, t, wc| match t.repo.clone() {
            Some(repo) => dialogs::ignore(w, t, repo, pattern),
            None => generic::ignore(w, t, wc, pattern),
        });
    });
    w.win.add_action(&a);
}
