//! Smart Move (README: Links): after a folder moved from `old` to `new` within a filesystem, the
//! links inside it are made to reach what they reached before. Absolute links pointing into the
//! folder point at its new place; relative links pointing out of it, which the move bent, are
//! rewritten to reach the same outside target; relative links within the folder and absolute
//! links out of it are right as they are and stay untouched.

use super::{make, resolve};
use std::path::{Path, PathBuf};

/// What Smart Move changed, and what it could not.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Fixed {
    pub links: Vec<PathBuf>,
    pub errors: Vec<(PathBuf, String)>,
}

/// Puts the links inside `new` (moved from `old`) right; `relative` spells rewritten links
/// relative where they can be; `backup` (Backup Mode) keeps the owner and attributes of each
/// link rewritten.
pub fn after_move(old: &Path, new: &Path, relative: bool, backup: bool) -> Fixed {
    let mut fixed = Fixed::default();
    let (old, new) = (super::lexical(old), super::lexical(new));
    let mut stack = vec![new.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
            let junction = make::is_junction(&p);
            if !(meta.file_type().is_symlink() || junction) {
                if meta.is_dir() {
                    stack.push(p);
                }
                continue;
            }
            let Ok(raw) = std::fs::read_link(&p) else { continue };
            let Ok(rest) = p.strip_prefix(&new) else { continue };
            let before = resolve(&old.join(rest), &raw);
            let want = match before.strip_prefix(&old) {
                Ok(inside) if inside.as_os_str().is_empty() => new.clone(),
                Ok(inside) => new.join(inside),
                Err(_) => before,
            };
            if resolve(&p, &raw) == want {
                continue;
            }
            match make::retarget_with(&p, &want, relative, backup) {
                Ok(()) => fixed.links.push(p),
                Err(e) => fixed.errors.push((p, e.to_string())),
            }
        }
    }
    fixed.links.sort();
    fixed
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn links_inside_a_moved_folder_reach_what_they_reached_before() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("a/proj");
        std::fs::create_dir_all(old.join("sub")).unwrap();
        std::fs::write(old.join("sub/f"), "f").unwrap();
        std::fs::create_dir_all(t.path().join("shared")).unwrap();
        std::fs::write(t.path().join("shared/s"), "s").unwrap();
        // Absolute into the folder, relative inside it, relative out of it, absolute out of it.
        symlink(old.join("sub/f"), old.join("abs-in")).unwrap();
        symlink("sub/f", old.join("rel-in")).unwrap();
        symlink("../../shared/s", old.join("rel-out")).unwrap();
        symlink(t.path().join("shared/s"), old.join("abs-out")).unwrap();
        let new = t.path().join("deeper/still/proj");
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::rename(&old, &new).unwrap();

        let fixed = after_move(&old, &new, true, false);
        assert!(fixed.errors.is_empty(), "{:?}", fixed.errors);
        assert_eq!(fixed.links, [new.join("abs-in"), new.join("rel-out")]);
        for n in ["abs-in", "rel-in"] {
            assert_eq!(std::fs::read_to_string(new.join(n)).unwrap(), "f", "{n}");
        }
        for n in ["rel-out", "abs-out"] {
            assert_eq!(std::fs::read_to_string(new.join(n)).unwrap(), "s", "{n}");
        }
        assert_eq!(std::fs::read_link(new.join("abs-in")).unwrap(), Path::new("sub/f"));
        assert_eq!(std::fs::read_link(new.join("rel-out")).unwrap(), Path::new("../../../shared/s"));
        assert_eq!(std::fs::read_link(new.join("abs-out")).unwrap(), t.path().join("shared/s"), "untouched");
    }
}
