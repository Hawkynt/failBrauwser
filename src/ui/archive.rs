//! Working inside archives from the UI: opening entries with applications (edits are saved
//! back into the archive), copying and moving in and out, deleting, renaming, creating,
//! extracting and compressing. Every archive operation runs as a job.

use super::clipboard::ClipSource;
use super::fileops;
use super::jobs::Outcome;
use super::model::Item;
use super::pane::Pane;
use super::util;
use super::window::Window;
use failbrauwser::archive::vfs::Vfs;
use failbrauwser::archive::{ArchiveError, is_browsable_name};
use failbrauwser::location::{ArchiveLoc, Location};
use failbrauwser::ops::copy::{self, Mode};
use failbrauwser::ops::job::{Answer, JobCtx, Question};
use failbrauwser::ops::{delete, names, staging};
#[cfg(windows)]
use failbrauwser::remote_fs;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

pub(super) fn io(e: ArchiveError) -> std::io::Error {
    std::io::Error::other(e.message)
}

/// Marks a failure for want of a password (the progress window then does not report it as
/// an error; the operation asks for the password and runs again).
pub const PASSWORD_MARK: &str = "\u{1}password\u{1}";

/// Like [`io`], but a missing or wrong password names the archive that needs it.
fn io_at(loc: &ArchiveLoc) -> impl Fn(ArchiveError) -> std::io::Error + '_ {
    move |e| {
        if e.needs_password() {
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{PASSWORD_MARK}{}\n{}", Location::Archive(loc.with_inner("")).display(), e.message))
        } else {
            io(e)
        }
    }
}

/// The archive whose password is missing, if that is why the operation failed.
fn password_failure(outcome: &Outcome) -> Option<ArchiveLoc> {
    let Outcome::Failed(msg) = outcome else { return None };
    let rest = msg.strip_prefix(PASSWORD_MARK)?;
    let where_ = rest.lines().next()?;
    match Location::parse(where_, is_browsable_name)? {
        Location::Archive(a) => Some(a.with_inner("")),
        _ => None,
    }
}

/// Asks for the password of `loc`, then runs `retry` (nothing on cancel).
fn ask_and_retry(w: &Window, loc: ArchiveLoc, wrong: bool, retry: impl Fn(&Window) + 'static) {
    let weak = Rc::downgrade(&w.me());
    let title = Location::Archive(loc.clone()).title();
    let label = if wrong { format!("Wrong password for “{title}”. Try again:") } else { format!("Password for “{title}”:") };
    util::ask_secret(&w.win, "Encrypted Archive", &label, move |pw| {
        Vfs::global().set_password(&loc, &pw);
        if let Some(w) = weak.upgrade() {
            retry(&w);
        }
    });
}

/// Runs archive work with the helper's progress shown in the job.
pub(super) fn tracked<T>(ctx: &JobCtx, f: impl FnOnce() -> T) -> T {
    use std::sync::atomic::{AtomicBool, Ordering};
    // Cancel stops the helper itself: it may be busy (or stuck) without reporting progress.
    let finished = AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            while !finished.load(Ordering::SeqCst) {
                if ctx.is_cancelled() {
                    Vfs::global().helper().abort();
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let r = tracked_progress(ctx, f);
        finished.store(true, Ordering::SeqCst);
        r
    })
}

fn tracked_progress<T>(ctx: &JobCtx, f: impl FnOnce() -> T) -> T {
    use std::sync::atomic::Ordering;
    // The job context outlives this call; the sink is dropped before it returns.
    let ctx_ptr = ctx as *const JobCtx as usize;
    failbrauwser::archive::client::with_progress(
        move |p| {
            let ctx = unsafe { &*(ctx_ptr as *const JobCtx) };
            if p.phase == "writing" {
                // Everything read; the archive is being written: an activity bar.
                ctx.bytes_total.store(0, Ordering::Relaxed);
                ctx.set_current("Writing the archive…");
            } else {
                ctx.bytes_total.store(p.total, Ordering::Relaxed);
                ctx.bytes_done.store(p.done, Ordering::Relaxed);
                ctx.set_current(match p.phase.as_str() {
                    "extracting" => "Extracting…",
                    "adding" => "Reading files for the archive…",
                    "removing" => "Rewriting the archive…",
                    _ => "Working…",
                });
            }
        },
        f,
    )
}

fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn parent_of(path: &str) -> &str {
    path.rfind('/').map(|i| &path[..i]).unwrap_or("")
}

/// The archive folder a tab shows, if it shows one.
fn archive_of(loc: &Location) -> Option<&ArchiveLoc> {
    match loc {
        Location::Archive(a) => Some(a),
        _ => None,
    }
}

/// Refreshes every tab that shows a folder of this archive (after edits the file monitor
/// catches the outer file; nested archives need a nudge too).
fn refresh_archive_tabs(w: &Window, a: &ArchiveLoc) {
    for win in w.app.windows() {
        for p in win.panes() {
            if let Location::Archive(x) = p.location() {
                if x.file == a.file {
                    p.schedule_refresh();
                }
            }
        }
    }
}

// ── Opening entries ─────────────────────────────────────────────────

thread_local! {
    /// Files extracted for an application; their monitors save edits back.
    static OPENED: RefCell<Vec<gio::FileMonitor>> = const { RefCell::new(Vec::new()) };
}

pub fn open_entry(w: &Window, item: &Item) {
    let (Item::Archive(node), Location::Archive(loc)) = (item, w.current_location()) else { return };
    if node.is_dir {
        return;
    }
    let entry = node.path.clone();
    let watched_entry = entry.clone();
    let retry_item = item.clone();
    let loc2 = loc.clone();
    let weak = Rc::downgrade(&w.me());
    let title = format!("Opening “{}”", node.name);
    w.app.jobs.start(
        w.win.upcast_ref(),
        title,
        move |ctx| {
            let v = Vfs::global();
            let dir = v.scratch_dir("open").map_err(io)?;
            tracked(ctx, || v.extract(&loc2, std::slice::from_ref(&entry), &dir)).map_err(io_at(&loc2))
        },
        move |outcome| {
            if let (Some(w), Some(pl)) = (weak.upgrade(), password_failure(outcome)) {
                let (item, wrong) = (retry_item.clone(), Vfs::global().has_password(&pl));
                ask_and_retry(&w, pl, wrong, move |w| open_entry(w, &item));
                return;
            }
            let (Some(w), Outcome::Done(paths)) = (weak.upgrade(), outcome) else { return };
            let Some(path) = paths.first().cloned() else { return };
            util::open_with_default(w.win.upcast_ref(), &path);
            watch_opened(&w, loc, watched_entry, path);
        },
    );
}

/// Saves an extracted file back into its archive whenever an application writes it.
fn watch_opened(w: &Window, loc: ArchiveLoc, entry: String, path: PathBuf) {
    if entry.is_empty() {
        return;
    }
    let Ok(monitor) = gio::File::for_path(&path).monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else { return };
    let pending = Rc::new(std::cell::Cell::new(false));
    let weak = Rc::downgrade(&w.me());
    monitor.connect_changed(move |_, _, _, ev| {
        use gio::FileMonitorEvent as E;
        if !matches!(ev, E::ChangesDoneHint | E::Created | E::MovedIn | E::Renamed | E::Changed) || pending.replace(true) {
            return;
        }
        let (weak, loc, entry, path, pending) = (weak.clone(), loc.clone(), entry.clone(), path.clone(), pending.clone());
        // Editors write in bursts; save once it settled.
        glib::timeout_add_local_once(Duration::from_millis(800), move || {
            pending.set(false);
            let Some(w) = weak.upgrade() else { return };
            if !path.is_file() {
                return;
            }
            let (loc2, entry2, path2) = (loc.clone(), entry.clone(), path.clone());
            let w2 = Rc::downgrade(&w);
            let name = leaf(&entry).to_string();
            w.app.jobs.start(
                w.win.upcast_ref(),
                format!("Saving “{name}” into the archive"),
                move |ctx| tracked(ctx, || Vfs::global().add(&loc2, &[(path2, entry2)])).map(|_| Vec::new()).map_err(io),
                move |outcome| {
                    if let Some(w) = w2.upgrade() {
                        if matches!(outcome, Outcome::Done(_)) {
                            w.set_status(&format!("Saved changes to “{name}” into the archive."));
                        }
                        refresh_archive_tabs(&w, &loc);
                    }
                },
            );
        });
    });
    OPENED.with(|o| o.borrow_mut().push(monitor));
}

// ── Transfers ───────────────────────────────────────────────────────

/// Resolves name clashes with what the archive folder already holds. Returns the entry
/// names to write (`None` = skip).
fn plan_names(ctx: &JobCtx, a: &ArchiveLoc, sources: &[(PathBuf, String)]) -> std::io::Result<Vec<Option<String>>> {
    let archive = Vfs::global().archive(a).map_err(io)?;
    let mut out = Vec::new();
    for (src, name) in sources {
        let target = a.entry_path(name);
        let Some(existing) = archive.tree.get(&target) else {
            out.push(Some(target));
            continue;
        };
        let meta = std::fs::metadata(src).ok();
        let q = Question::Conflict {
            src: src.clone(),
            dst: Location::Archive(a.with_inner(&target)).display().into(),
            src_size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            dst_size: existing.size,
            src_mtime: meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0),
            dst_mtime: existing.mtime.unwrap_or(0),
            dst_is_dir: existing.is_dir,
        };
        match ctx.ask(q) {
            Answer::Replace => out.push(Some(target)),
            Answer::KeepBoth => {
                let free = names::free_name_in(&|n| archive.tree.get(&a.entry_path(&n.to_string_lossy())).is_some(), OsStr::new(name), None);
                out.push(Some(a.entry_path(&free.to_string_lossy())));
            }
            Answer::Cancel => return Err(failbrauwser::ops::fastcopy::cancelled()),
            _ => out.push(None),
        }
    }
    Ok(out)
}

/// Copies or moves anything involving an archive (the plain folder-to-folder case is
/// handled by the regular copy engine).
pub fn transfer(w: &Window, sources: Vec<ClipSource>, dest: Location, mode: Mode, after: Box<dyn FnOnce()>) {
    let verb = if mode == Mode::Move { "Moving" } else { "Copying" };
    let n = sources.len();
    let title = format!("{verb} {n} item{} to “{}”", if n == 1 { "" } else { "s" }, dest.title());
    let opts = copy::Options { smooth_writes: w.app.settings.borrow().smooth_writes, workers: None, verify: w.app.settings.borrow().verify_copies };
    let weak = Rc::downgrade(&w.me());
    let dest2 = dest.clone();
    let touched: Vec<ArchiveLoc> = sources
        .iter()
        .filter_map(|s| match s {
            ClipSource::Archive(a, _) => Some(a.clone()),
            _ => None,
        })
        .chain(archive_of(&dest).cloned())
        .collect();
    let retry_sources = sources.clone();
    #[cfg(windows)]
    let touched_servers: Vec<failbrauwser::location::RemoteAt> = sources
        .iter()
        .filter_map(|s| match s {
            ClipSource::Remote(t, ..) => Some(t.clone()),
            _ => None,
        })
        .chain(match &dest {
            Location::Remote(r) => Some(r.at.clone()),
            _ => None,
        })
        .collect();
    w.app.jobs.start(
        w.win.upcast_ref(),
        title,
        move |ctx| transfer_work(ctx, sources, &dest2, mode, &opts),
        move |outcome| {
            if let (Some(w), Some(pl)) = (weak.upgrade(), password_failure(outcome)) {
                let wrong = Vfs::global().has_password(&pl);
                let (src, dst) = (retry_sources.clone(), dest.clone());
                let after = std::cell::RefCell::new(Some(after));
                ask_and_retry(&w, pl, wrong, move |w| {
                    if let Some(a) = after.borrow_mut().take() {
                        transfer(w, src.clone(), dst.clone(), mode, a);
                    }
                });
                return;
            }
            after();
            let Some(w) = weak.upgrade() else { return };
            for a in &touched {
                refresh_archive_tabs(&w, a);
            }
            #[cfg(windows)]
            for t in &touched_servers {
                refresh_remote_tabs(&w, t);
            }
            if let Outcome::Done(created) = outcome {
                fileops::select_created(&w, &dest, created);
            }
        },
    );
}

fn transfer_work(ctx: &JobCtx, sources: Vec<ClipSource>, dest: &Location, mode: Mode, opts: &copy::Options) -> std::io::Result<Vec<PathBuf>> {
    let v = Vfs::global();
    let mut created = Vec::new();
    // Moves inside one archive are renames.
    if let (Location::Archive(d), Mode::Move) = (dest, mode) {
        let same: Vec<String> = sources
            .iter()
            .filter_map(|s| match s {
                ClipSource::Archive(a, e) if a.archive_key() == d.archive_key() => Some(e.clone()),
                _ => None,
            })
            .collect();
        if same.len() == sources.len() {
            for e in same {
                if parent_of(&e) == d.inner || d.inner == e || d.inner.starts_with(&format!("{e}/")) {
                    continue;
                }
                let to = d.entry_path(leaf(&e));
                ctx.set_current(leaf(&e).to_string());
                v.rename(d, &e, &to).map_err(io)?;
                created.push(PathBuf::from(leaf(&e)));
            }
            return Ok(created);
        }
    }

    // Everything coming out of an archive is extracted to a staging folder first: next to
    // the destination when that is a folder (so the final step is a rename), else in the cache.
    // Network shares get a local one: the helper writes in parallel, which hangs gvfsd-fuse.
    let staging = match dest {
        Location::Dir(d) if copy::is_gvfs_path(d) => local_staging()?,
        Location::Dir(d) => {
            let s = tempfile::Builder::new().prefix(".fb-extract-").tempdir_in(d)?;
            s.keep()
        }
        _ => v.scratch_dir("stage").map_err(io)?,
    };
    // Recorded, so a crash in between does not leave it behind for good.
    staging::created(&staging);
    let result = (|| {
        let mut local: Vec<PathBuf> = Vec::new();
        let mut from_archives: Vec<(ArchiveLoc, String)> = Vec::new();
        #[cfg(windows)]
        let mut from_servers: Vec<(failbrauwser::location::RemoteAt, String, bool)> = Vec::new();
        for s in &sources {
            match s {
                ClipSource::Local(p) => local.push(p.clone()),
                ClipSource::Archive(a, e) => {
                    ctx.set_current(format!("Extracting {}", leaf(e)));
                    let got = tracked(ctx, || v.extract(a, std::slice::from_ref(e), &staging)).map_err(io_at(a))?;
                    local.extend(got);
                    from_archives.push((a.clone(), e.clone()));
                }
                // Remote entries are pulled to staging whole (folders walked), exactly
                // like archive extractions: from there the destination legs are the usual ones.
                #[cfg(windows)]
                ClipSource::Remote(t, e, is_dir) => {
                    ctx.set_current(format!("Downloading {}", leaf(e)));
                    local.extend(remote_fs::fetch_tree(ctx, t, e, *is_dir, &staging)?);
                    from_servers.push((t.clone(), e.clone(), *is_dir));
                }
                #[cfg(not(windows))]
                ClipSource::Remote(..) => return Err(std::io::Error::other("remote copies run where the app's own remote stack runs — on this platform those addresses belong to the desktop's mounts")),
            }
        }
        match dest {
            Location::Dir(d) => {
                // Extracted items move out of staging (same filesystem: renames); local
                // sources copy or move as usual. Both ask about conflicts.
                let (staged, plain): (Vec<PathBuf>, Vec<PathBuf>) = local.into_iter().partition(|p| p.starts_with(&staging));
                if !staged.is_empty() {
                    created.extend(copy::transfer(ctx, &staged, d, Mode::Move, opts)?);
                }
                if !plain.is_empty() {
                    created.extend(copy::transfer(ctx, &plain, d, mode, opts)?);
                }
            }
            Location::Archive(d) => {
                let items: Vec<(PathBuf, String)> = local.iter().map(|p| (p.clone(), p.file_name().unwrap_or_default().to_string_lossy().into_owned())).collect();
                let targets = plan_names(ctx, d, &items)?;
                let add: Vec<(PathBuf, String)> = items.iter().zip(&targets).filter_map(|((p, _), t)| t.clone().map(|t| (p.clone(), t))).collect();
                if !add.is_empty() {
                    ctx.set_current(format!("Adding {} item{} to the archive", add.len(), if add.len() == 1 { "" } else { "s" }));
                    tracked(ctx, || v.add(d, &add)).map_err(io_at(d))?;
                }
                created.extend(add.iter().map(|(_, t)| PathBuf::from(leaf(t))));
                if mode == Mode::Move {
                    let moved_local: Vec<PathBuf> = local.iter().zip(&targets).filter(|(p, t)| t.is_some() && !p.starts_with(&staging)).map(|(p, _)| p.clone()).collect();
                    delete::delete_permanently(ctx, &moved_local)?;
                }
            }
            // Pasting into a server folder: local sources go out over the wire as-is,
            // staged ones (from archives or other servers) the same way; conflicts ask
            // the job's own question (see remote_fs::publish_tree).
            #[cfg(windows)]
            Location::Remote(d) => {
                created.extend(remote_fs::publish_tree(ctx, &local, &d.at, &d.inner)?);
                if mode == Mode::Move {
                    let moved_local: Vec<PathBuf> = local.iter().filter(|p| !p.starts_with(&staging)).cloned().collect();
                    delete::delete_permanently(ctx, &moved_local)?;
                }
            }
            #[cfg(not(windows))]
            Location::Remote(_) => return Err(std::io::Error::other("remote copies run where the app's own remote stack runs — on this platform those addresses belong to the desktop's mounts")),
            Location::Drives | Location::Trash | Location::Search(_) | Location::Network(_) => {}
        }
        if mode == Mode::Move {
            for (a, e) in &from_archives {
                ctx.set_current(format!("Removing {} from the archive", leaf(e)));
                tracked(ctx, || v.remove(a, std::slice::from_ref(e))).map_err(io)?;
            }
            // Server-side removals are permanent (no trash on the other machine's folder).
            #[cfg(windows)]
            for (t, e, is_dir) in &from_servers {
                ctx.set_current(format!("Removing {} from the server", leaf(e)));
                remote_fs::remove(t, e, *is_dir)?;
            }
        }
        Ok(created)
    })();
    staging::remove(&staging);
    result
}

// ── Delete, rename, create ──────────────────────────────────────────

pub fn delete_selection(w: &Window, pane: &Rc<Pane>) {
    let Location::Archive(a) = pane.location() else { return };
    let entries: Vec<String> = pane.selected_items().iter().filter_map(|i| match i {
        Item::Archive(n) => Some(n.path.clone()),
        _ => None,
    }).collect();
    if entries.is_empty() {
        return;
    }
    let weak = Rc::downgrade(&w.me());
    let a2 = a.clone();
    w.app.jobs.start(
        w.win.upcast_ref(),
        format!("Deleting {} item{} from the archive", entries.len(), if entries.len() == 1 { "" } else { "s" }),
        move |ctx| tracked(ctx, || Vfs::global().remove(&a2, &entries)).map(|_| Vec::new()).map_err(io),
        move |_| {
            if let Some(w) = weak.upgrade() {
                refresh_archive_tabs(&w, &a);
            }
        },
    );
}

pub fn rename(w: &Window, pane: &Rc<Pane>, item: &Item, new: &str) {
    let (Location::Archive(a), Item::Archive(n)) = (pane.location(), item) else { return };
    let from = n.path.clone();
    let to = a.entry_path(new);
    if pane.has_name(new) {
        util::show_error(&w.win, "Cannot rename", &format!("“{new}” already exists."));
        return;
    }
    let weak = Rc::downgrade(&w.me());
    let name = new.to_string();
    let a2 = a.clone();
    w.app.jobs.start(
        w.win.upcast_ref(),
        format!("Renaming “{}”", n.name),
        move |ctx| tracked(ctx, || Vfs::global().rename(&a2, &from, &to)).map(|_| Vec::new()).map_err(io),
        move |outcome| {
            if let Some(w) = weak.upgrade() {
                refresh_archive_tabs(&w, &a);
                if matches!(outcome, Outcome::Done(_)) {
                    w.current_pane().select_when_present(vec![name.clone().into()]);
                }
            }
        },
    );
}

/// Renames over an entry that is there: the entry in the way is deleted first.
pub fn rename_replacing(w: &Window, pane: &Rc<Pane>, item: &Item, new: &str) {
    let (Location::Archive(a), Item::Archive(n)) = (pane.location(), item) else { return };
    let from = n.path.clone();
    let to = a.entry_path(new);
    let weak = Rc::downgrade(&w.me());
    let name = new.to_string();
    let a2 = a.clone();
    w.app.jobs.start(
        w.win.upcast_ref(),
        format!("Renaming “{}”", n.name),
        move |ctx| {
            tracked(ctx, || {
                let v = Vfs::global();
                v.remove(&a2, std::slice::from_ref(&to))?;
                v.rename(&a2, &from, &to)
            })
            .map(|_| Vec::new())
            .map_err(io)
        },
        move |outcome| {
            if let Some(w) = weak.upgrade() {
                refresh_archive_tabs(&w, &a);
                if matches!(outcome, Outcome::Done(_)) {
                    w.current_pane().select_when_present(vec![name.clone().into()]);
                }
            }
        },
    );
}

pub fn create(w: &Window, loc: &Location, name: &str, folder: bool) {
    let Location::Archive(a) = loc.clone() else { return };
    let path = a.entry_path(name);
    let weak = Rc::downgrade(&w.me());
    let name2 = name.to_string();
    let a2 = a.clone();
    w.app.jobs.start(
        w.win.upcast_ref(),
        format!("Creating “{name}” in the archive"),
        move |_| {
            let v = Vfs::global();
            if folder {
                v.mkdir(&a2, &path).map_err(io)?;
            } else {
                let dir = v.scratch_dir("new").map_err(io)?;
                staging::created(&dir);
                let file = dir.join("empty");
                std::fs::write(&file, b"")?;
                let r = v.add(&a2, &[(file, path)]).map_err(io);
                staging::remove(&dir);
                r?;
            }
            Ok(Vec::new())
        },
        move |outcome| {
            if let Some(w) = weak.upgrade() {
                refresh_archive_tabs(&w, &a);
                if matches!(outcome, Outcome::Done(_)) {
                    w.current_pane().select_when_present(vec![name2.clone().into()]);
                }
            }
        },
    );
}

/// Total sizes of the folders shown in an archive (known from the listing, no scanning).
pub fn fill_sizes(pane: &Rc<Pane>) {
    let Location::Archive(a) = pane.location() else { return };
    let Some(archive) = Vfs::global().cached(&a) else { return };
    let sizes: Vec<(std::ffi::OsString, u64)> = pane
        .dir_names()
        .into_iter()
        .map(|name| {
            let total = archive.tree.total_size(&a.entry_path(&name.to_string_lossy()));
            (name, total)
        })
        .collect();
    pane.set_sizes(&sizes);
}

// ── Extract, compress, open as archive ──────────────────────────────

/// A staging folder on local disk (not in the runtime folder, which is memory), for
/// extractions bound for a network share.
fn local_staging() -> std::io::Result<PathBuf> {
    let base = glib::user_cache_dir().join("failbrauwser").join("stage");
    std::fs::create_dir_all(&base)?;
    let dir = tempfile::Builder::new().prefix("fb-").tempdir_in(&base)?.keep();
    staging::created(&dir);
    Ok(dir)
}

/// Re-read every tab showing a folder on this server (its contents changed at the source
/// or destination of a transfer).
#[cfg(windows)]
fn refresh_remote_tabs(w: &Window, t: &failbrauwser::location::RemoteAt) {
    for win in w.app.windows() {
        for p in win.panes() {
            if let Location::Remote(r) = p.location() {
                if r.at == *t {
                    p.schedule_refresh();
                }
            }
        }
    }
}

/// Archive files among the selection (local files only).
pub fn selected_archives(pane: &Pane) -> Vec<PathBuf> {
    pane.selected_items().iter().filter_map(|i| i.path().filter(|_| !i.is_dir_like() && is_browsable_name(&i.display_name())).cloned()).collect()
}

pub fn extract_selected(w: &Window, dest: Option<PathBuf>) {
    let pane = w.current_pane();
    let archives = selected_archives(&pane);
    let Some(dest) = dest.or_else(|| pane.location().local_path().map(Path::to_path_buf)) else { return };
    extract_archives(w, archives, dest);
}

fn extract_archives(w: &Window, archives: Vec<PathBuf>, dest: PathBuf) {
    if archives.is_empty() {
        return;
    }
    let (retry_archives, retry_dest) = (archives.clone(), dest.clone());
    let weak = Rc::downgrade(&w.me());
    let dest_loc = Location::Dir(dest.clone());
    w.app.jobs.start(
        w.win.upcast_ref(),
        format!("Extracting {} archive{}", archives.len(), if archives.len() == 1 { "" } else { "s" }),
        move |ctx| {
            let mut made = Vec::new();
            for a in &archives {
                ctx.set_current(a.file_name().unwrap_or_default().to_string_lossy());
                let loc = ArchiveLoc::root(a.clone());
                if copy::is_gvfs_path(&dest) {
                    // Extracted locally, then copied onto the share (see `transfer_work`).
                    let staging = local_staging()?;
                    let result = tracked(ctx, || Vfs::global().extract_all(&loc, &staging))
                        .map_err(io_at(&loc))
                        .and_then(|got| copy::transfer(ctx, &[got], &dest, Mode::Move, &copy::Options::default()));
                    staging::remove(&staging);
                    made.extend(result?);
                } else {
                    made.push(tracked(ctx, || Vfs::global().extract_all(&loc, &dest)).map_err(io_at(&loc))?);
                }
            }
            Ok(made)
        },
        move |outcome| {
            if let (Some(w), Some(pl)) = (weak.upgrade(), password_failure(outcome)) {
                let wrong = Vfs::global().has_password(&pl);
                let (a, d) = (retry_archives.clone(), retry_dest.clone());
                ask_and_retry(&w, pl, wrong, move |w| extract_archives(w, a.clone(), d.clone()));
                return;
            }
            if let (Some(w), Outcome::Done(made)) = (weak.upgrade(), outcome) {
                fileops::select_created(&w, &dest_loc, made);
            }
        },
    );
}

pub fn extract_to(w: &Window) {
    let chooser = gtk::FileChooserNative::new(Some("Extract To"), Some(&w.win), gtk::FileChooserAction::SelectFolder, Some("_Extract"), Some("_Cancel"));
    if let Some(dir) = w.current_location().nearest_dir() {
        let _ = chooser.set_current_folder(dir);
    }
    let weak = Rc::downgrade(&w.me());
    chooser.connect_response(move |c, resp| {
        if resp == gtk::ResponseType::Accept {
            if let (Some(w), Some(dir)) = (weak.upgrade(), c.filename()) {
                extract_selected(&w, Some(dir));
            }
        }
    });
    chooser.show();
}

pub fn compress(w: &Window) {
    super::compress::show(w);
}

/// Opens a file as a folder when the archive library can read it, else with an application.
pub fn enter_or_open(w: &Window, path: PathBuf) {
    let me = Rc::downgrade(&w.me());
    let pane = Rc::downgrade(&w.current_pane());
    glib::spawn_future_local(async move {
        // It has to list something: a name alone (random data called ".bin") is not enough.
        let probe = path.clone();
        let readable = gio::spawn_blocking(move || match Vfs::global().helper().list(&probe, None) {
            Ok(listing) => !listing.entries.is_empty(),
            Err(e) => e.needs_password(),
        })
        .await
        .unwrap_or(false);
        let (Some(w), Some(pane)) = (me.upgrade(), pane.upgrade()) else { return };
        if readable {
            pane.navigate(Location::Archive(ArchiveLoc::root(path)));
        } else {
            util::open_with_default(w.win.upcast_ref(), &path);
        }
    });
}

/// Opens any file as a folder if the archive library understands it.
pub fn open_as_archive(w: &Window) {
    let pane = w.current_pane();
    let Some(item) = pane.selected_items().into_iter().next() else { return };
    let loc = pane.location();
    let target = match (&loc, &item) {
        (_, Item::Fs(e)) if !e.is_dir_like() => Location::Archive(ArchiveLoc::root(e.path.clone())),
        (Location::Archive(a), Item::Archive(n)) if !n.is_dir => Location::Archive(a.enter_nested(&n.path)),
        _ => return,
    };
    let probe_path = item.path().cloned();
    let me = Rc::downgrade(&w.me());
    glib::spawn_future_local(async move {
        // Plain files are probed first so a non-archive gives a clear message.
        if let Some(p) = probe_path {
            let res = gio::spawn_blocking(move || Vfs::global().helper().probe(&p)).await;
            if !matches!(res, Ok(Ok((true, _, _)))) {
                if let Some(w) = me.upgrade() {
                    util::show_error(&w.win, "Not an archive", "This file cannot be opened as a folder.");
                }
                return;
            }
        }
        if let Some(w) = me.upgrade() {
            w.navigate(target);
        }
    });
}

thread_local! {
    static ASKED: std::cell::RefCell<std::collections::HashSet<(PathBuf, Vec<String>)>> = std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Entering an archive with encrypted entries asks for the password once.
pub fn maybe_ask_password(w: &Window, pane: &Rc<Pane>) {
    let loc = pane.location();
    let Location::Archive(a) = &loc else { return };
    let v = Vfs::global();
    if v.has_password(a) || !v.cached(a).is_some_and(|x| x.tree.any_encrypted()) {
        return;
    }
    if !ASKED.with(|s| s.borrow_mut().insert(a.archive_key())) {
        return;
    }
    ask_password(w, pane, &loc);
}

pub fn ask_password(w: &Window, pane: &Rc<Pane>, loc: &Location) {
    let Location::Archive(a) = loc else { return };
    let key = a.clone();
    let p = Rc::downgrade(pane);
    util::ask_secret(&w.win, "Encrypted Archive", &format!("Password for “{}”:", loc.title()), move |pw| {
        Vfs::global().set_password(&key, &pw);
        if let Some(p) = p.upgrade() {
            p.reload();
        }
    });
}
