//! What an operation did, kept so that Ctrl+Z can take it back: renames and moves go back
//! where they came from, copies go to the trash, new empty folders and files are removed
//! again, and what went to the trash comes back out of it.

use super::copy::{self, Mode};
use super::job::JobCtx;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Renamed or moved: (where it was, where it is now).
    Moved(Vec<(PathBuf, PathBuf)>),
    /// Copies that were made.
    Copied(Vec<PathBuf>),
    /// A new folder or file.
    Created(PathBuf),
    /// Moved to the trash from these places.
    Trashed(Vec<PathBuf>),
}

/// An operation that can be taken back, and what to call it ("Rename “a.txt”").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Undoable {
    pub what: String,
    pub step: Step,
}

/// Takes `step` back. What could not be (moved away since, a name taken again) is recorded
/// in `ctx` as a skipped item. Returns the paths that are back.
pub fn undo(ctx: &JobCtx, step: &Step) -> io::Result<Vec<PathBuf>> {
    match step {
        Step::Moved(pairs) => {
            let mut back = Vec::new();
            for (was, now) in pairs.iter().rev() {
                if std::fs::symlink_metadata(now).is_err() {
                    ctx.record_error(format!("“{}” is no longer there.", now.display()));
                    continue;
                }
                if std::fs::symlink_metadata(was).is_ok() {
                    ctx.record_error(format!("“{}” exists again; “{}” stays where it is.", was.display(), now.display()));
                    continue;
                }
                match copy::rename_noreplace(now, was) {
                    Ok(()) => back.push(was.clone()),
                    // Another drive: moved back by copying.
                    Err(e) if crate::osprim::is_cross_device(&e) => back.extend(move_across(ctx, now, was)?),
                    Err(e) => ctx.record_error(format!("{}: {e}", now.display())),
                }
            }
            Ok(back)
        }
        Step::Copied(paths) => {
            let there: Vec<PathBuf> = paths.iter().filter(|p| std::fs::symlink_metadata(p).is_ok()).cloned().collect();
            super::delete::trash(ctx, &there)?;
            Ok(Vec::new())
        }
        Step::Created(p) => {
            let m = std::fs::symlink_metadata(p)?;
            let empty = if m.is_dir() { std::fs::read_dir(p)?.next().is_none() } else { m.len() == 0 };
            if empty {
                if m.is_dir() { std::fs::remove_dir(p)? } else { std::fs::remove_file(p)? }
            } else {
                // Something was put into it since: kept in the trash, not lost.
                super::delete::trash(ctx, std::slice::from_ref(p))?;
            }
            Ok(Vec::new())
        }
        Step::Trashed(paths) => {
            let items = crate::trash::list(&crate::trash::trash_dirs());
            let mut back = Vec::new();
            for p in paths {
                // The newest one, if the same place was trashed more than once.
                let Some(item) = items.iter().filter(|i| i.original == *p).max_by_key(|i| i.deleted_unix()) else {
                    ctx.record_error(format!("“{}” is no longer in the trash.", p.display()));
                    continue;
                };
                match crate::trash::restore(item, None) {
                    Ok(at) => back.push(at),
                    Err(e) => ctx.record_error(format!("{}: {e}", p.display())),
                }
            }
            Ok(back)
        }
    }
}

fn move_across(ctx: &JobCtx, now: &Path, was: &Path) -> io::Result<Vec<PathBuf>> {
    let dir = was.parent().ok_or_else(|| io::Error::other("no folder to go back to"))?;
    let opts = copy::Options { smooth_writes: true, workers: None, verify: false };
    let made = copy::transfer(ctx, &[now.to_path_buf()], dir, Mode::Move, &opts)?;
    let mut out = Vec::new();
    for m in made {
        // It had another name there (a rename and a move at once).
        if m != was && copy::rename_noreplace(&m, was).is_ok() {
            out.push(was.to_path_buf());
        } else {
            out.push(m);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_go_back_and_new_empty_things_go() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/moved.txt"), "m").unwrap();
        std::fs::write(d.join("renamed.txt"), "r").unwrap();
        let ctx = JobCtx::new();
        let step = Step::Moved(vec![(d.join("moved.txt"), d.join("sub/moved.txt")), (d.join("old.txt"), d.join("renamed.txt"))]);
        let back = undo(&ctx, &step).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(std::fs::read_to_string(d.join("moved.txt")).unwrap(), "m");
        assert_eq!(std::fs::read_to_string(d.join("old.txt")).unwrap(), "r");
        // Undone again: nothing is there to move back; that is reported, not an error.
        let ctx = JobCtx::new();
        assert!(undo(&ctx, &step).unwrap().is_empty());
        assert_eq!(ctx.errors().len(), 2);
        // A taken name is left alone.
        std::fs::write(d.join("a"), "new a").unwrap();
        std::fs::write(d.join("b"), "b").unwrap();
        let ctx = JobCtx::new();
        undo(&ctx, &Step::Moved(vec![(d.join("a"), d.join("b"))])).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("a")).unwrap(), "new a");
        assert!(d.join("b").exists());

        std::fs::create_dir(d.join("New Folder")).unwrap();
        std::fs::write(d.join("New Document.txt"), "").unwrap();
        undo(&JobCtx::new(), &Step::Created(d.join("New Folder"))).unwrap();
        undo(&JobCtx::new(), &Step::Created(d.join("New Document.txt"))).unwrap();
        assert!(!d.join("New Folder").exists() && !d.join("New Document.txt").exists());
    }
}
