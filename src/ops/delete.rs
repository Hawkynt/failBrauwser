//! Deleting: to the platform trash (freedesktop.org folders via GIO on Linux, the shell
//! Recycle Bin on Windows — both behind `trash::trash_file`) or permanently.

use super::fastcopy::{self, is_cancelled_error};
use super::job::{Answer, JobCtx, Phase, Question};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

fn count(p: &Path) -> u64 {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => 1 + fs::read_dir(p).map(|d| d.filter_map(Result::ok).map(|e| count(&e.path())).sum()).unwrap_or(0),
        Ok(_) => 1,
        Err(_) => 0,
    }
}

fn ask_error(ctx: &JobCtx, path: &Path, e: &io::Error) -> io::Result<bool> {
    match ctx.ask(Question::Error { path: path.to_path_buf(), message: e.to_string() }) {
        Answer::Retry => Ok(true),
        Answer::Cancel => Err(fastcopy::cancelled()),
        _ => {
            ctx.record_error(format!("{}: {}", path.display(), e));
            Ok(false)
        }
    }
}

fn remove_tree(ctx: &JobCtx, p: &Path) -> io::Result<()> {
    ctx.wait_while_paused();
    if ctx.is_cancelled() {
        return Err(fastcopy::cancelled());
    }
    let meta = match fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            ask_error(ctx, p, &e)?;
            return Ok(());
        }
    };
    ctx.set_current(p.file_name().unwrap_or_default().to_string_lossy());
    let mut sharing = 0u32;
    if meta.is_dir() {
        let entries = loop {
            match fs::read_dir(p).and_then(|d| d.collect::<io::Result<Vec<_>>>()) {
                Ok(v) => break v,
                Err(e) if fastcopy::wait_sharing_retry(&e, &mut sharing) => continue,
                Err(e) => {
                    sharing = 0;
                    if !ask_error(ctx, p, &e)? {
                        return Ok(());
                    }
                }
            }
        };
        for e in entries {
            remove_tree(ctx, &e.path())?;
        }
        loop {
            match fs::remove_dir(p) {
                Ok(()) => break,
                // Something inside was skipped; the folder stays.
                Err(e) if crate::osprim::is_dir_not_empty(&e) => break,
                Err(e) if fastcopy::wait_sharing_retry(&e, &mut sharing) => continue,
                Err(e) => {
                    sharing = 0;
                    if !ask_error(ctx, p, &e)? {
                        break;
                    }
                }
            }
        }
    } else {
        loop {
            match fs::remove_file(p) {
                Ok(()) => break,
                Err(e) if e.kind() == io::ErrorKind::NotFound => break,
                Err(e) if fastcopy::wait_sharing_retry(&e, &mut sharing) => continue,
                Err(e) => {
                    sharing = 0;
                    if !ask_error(ctx, p, &e)? {
                        break;
                    }
                }
            }
        }
    }
    ctx.files_done.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Deletes files and folder trees for good.
pub fn delete_permanently(ctx: &JobCtx, paths: &[PathBuf]) -> io::Result<()> {
    ctx.set_phase(Phase::Preparing);
    let total: u64 = paths.iter().map(|p| count(p)).sum();
    ctx.files_total.fetch_add(total, Ordering::Relaxed);
    ctx.set_phase(Phase::Working);
    for p in paths {
        match remove_tree(ctx, p) {
            Err(e) if is_cancelled_error(&e) => return Err(e),
            r => r?,
        }
    }
    ctx.set_phase(Phase::Done);
    Ok(())
}

/// Moves to the trash. Where there is no trash (some network shares), the user can
/// choose to delete permanently instead.
pub fn trash(ctx: &JobCtx, paths: &[PathBuf]) -> io::Result<()> {
    ctx.set_phase(Phase::Working);
    ctx.files_total.fetch_add(paths.len() as u64, Ordering::Relaxed);
    for p in paths {
        if ctx.is_cancelled() {
            return Err(fastcopy::cancelled());
        }
        ctx.set_current(p.file_name().unwrap_or_default().to_string_lossy());
        match crate::trash::trash_file(p) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => match ctx.ask(Question::TrashUnsupported { path: p.clone(), message: e.to_string() }) {
                Answer::DeleteInstead => remove_tree(ctx, p)?,
                Answer::Cancel => return Err(fastcopy::cancelled()),
                _ => ctx.record_error(format!("{}: {}", p.display(), e)),
            },
        }
        ctx.files_done.fetch_add(1, Ordering::Relaxed);
    }
    ctx.set_phase(Phase::Done);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)] // drives the live Recycle Bin through the delete operation — only ever its own file
    fn trashing_a_local_file_lands_in_the_recycle_bin() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("gone.txt");
        fs::write(&f, "x").unwrap();
        let ctx = JobCtx::new();
        trash(&ctx, &[f.clone()]).unwrap();
        assert!(!f.exists(), "the file must have left its original place");
        assert_eq!(ctx.snapshot().files_done, ctx.snapshot().files_total);
        if let Ok(items) = crate::win::recyclebin::list() {
            if let Some(item) = items.iter().find(|b| b.original == f) {
                crate::win::recyclebin::delete(&item.path).ok();
            }
        }
    }

    #[test]
    #[cfg(unix)] // the fixture builds a Unix symlink; no-follow delete is what it proves
    fn deletes_trees_without_following_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        fs::create_dir_all(d.join("t/a/b")).unwrap();
        fs::write(d.join("t/a/b/f"), "x").unwrap();
        fs::create_dir(d.join("outside")).unwrap();
        fs::write(d.join("outside/keep"), "x").unwrap();
        std::os::unix::fs::symlink(d.join("outside"), d.join("t/link")).unwrap();
        let ctx = JobCtx::new();
        delete_permanently(&ctx, &[d.join("t")]).unwrap();
        assert!(!d.join("t").exists());
        assert!(d.join("outside/keep").exists());
        assert_eq!(ctx.snapshot().files_done, ctx.snapshot().files_total);
    }
}
