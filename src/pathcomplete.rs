//! Completing a path typed into the location bar (README: Location bar): the folder being typed
//! in is listed, and what is typed after its last slash picks among its folders, and among the
//! files that open as folders (archives). Names match without regard to case; names starting
//! with a dot only once a dot is typed; Tab takes the text as far as all matches agree.

use std::io;
use std::path::{Path, PathBuf};

/// The most names of one folder offered; a folder with more lists its first ones.
pub const MAX: usize = 5000;

/// A typed path cut at its last slash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typed {
    /// The text up to and with the last slash, as typed (`~/Pro` → `~/`).
    pub dir_text: String,
    /// That folder (`~` made the home folder).
    pub dir: PathBuf,
    /// What is typed after the slash.
    pub name: String,
}

/// Whether the text is a path to complete: absolute, or starting at the home folder.
pub fn is_path(text: &str) -> bool {
    text.starts_with('/') || text == "~" || text.starts_with("~/")
}

/// The typed text cut at its last slash; `None` when it is no path. `~` alone is the home folder.
pub fn split(text: &str, home: &Path) -> Option<Typed> {
    if !is_path(text) {
        return None;
    }
    let text = if text == "~" { "~/" } else { text };
    let cut = text.rfind('/')? + 1;
    let dir_text = text[..cut].to_string();
    let dir = match dir_text.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(&dir_text),
    };
    Some(Typed { dir_text, dir, name: text[cut..].to_string() })
}

/// The names in `dir` a typed path can go on with: (name, is a folder), folders first, then the
/// files `is_archive` takes, each part sorted without regard to case. A network share that does
/// not answer is not waited for.
pub fn list(dir: &Path, is_archive: impl Fn(&str) -> bool) -> io::Result<Vec<(String, bool)>> {
    crate::ops::gvfs::check(dir, std::time::Duration::from_secs(2))?;
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(kind) = e.file_type() else { continue };
        // A link counts as what it points at.
        let is_dir = if kind.is_symlink() { e.path().is_dir() } else { kind.is_dir() };
        if is_dir || is_archive(&name) {
            out.push((name, is_dir));
        }
        if out.len() >= MAX {
            break;
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    Ok(out)
}

/// The full texts a listing offers after `dir_text`: folders end in a slash, so taking one goes
/// on into it.
pub fn texts(dir_text: &str, listing: &[(String, bool)]) -> Vec<String> {
    listing.iter().map(|(n, d)| format!("{dir_text}{n}{}", if *d { "/" } else { "" })).collect()
}

/// Whether the offered `candidate` fits what is typed: the same folder, and its name starting
/// with the typed name (any case). A name with a dot in front only for a typed dot; what is typed
/// already in full is no suggestion.
pub fn matches(candidate: &str, typed: &str) -> bool {
    let typed = if typed == "~" { "~/" } else { typed };
    let Some(cut) = typed.rfind('/').map(|i| i + 1) else { return false };
    let (dir, name) = typed.split_at(cut);
    let Some(rest) = candidate.strip_prefix(dir) else { return false };
    if rest.starts_with('.') && !name.starts_with('.') {
        return false;
    }
    let rest = rest.trim_end_matches('/');
    !rest.contains('/') && rest.to_lowercase().starts_with(&name.to_lowercase()) && candidate != typed
}

/// What Tab makes of `typed` with these offers: the one match in full, or the matches' common
/// beginning when that is longer than what is typed; `None` when there is nothing to add.
pub fn tab(typed: &str, offers: &[String]) -> Option<String> {
    let fits: Vec<&String> = offers.iter().filter(|c| matches(c, typed)).collect();
    match fits.as_slice() {
        [] => None,
        [one] => Some((*one).clone()),
        many => {
            let common = common_prefix(many);
            (common.chars().count() > typed.chars().count()).then_some(common)
        }
    }
}

/// The beginning all of `texts` share, compared without regard to case and spelled like the
/// first of them.
fn common_prefix(texts: &[&String]) -> String {
    let first: Vec<char> = texts[0].chars().collect();
    let mut n = first.len();
    for t in &texts[1..] {
        n = n.min(first.iter().zip(t.chars()).take_while(|(a, b)| a.to_lowercase().eq(b.to_lowercase())).count());
    }
    first[..n].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_path_is_cut_at_its_last_slash() {
        let home = Path::new("/home/me");
        assert_eq!(split("~/Pro", home), Some(Typed { dir_text: "~/".into(), dir: "/home/me/".into(), name: "Pro".into() }));
        assert_eq!(split("~", home).unwrap().dir, Path::new("/home/me/"));
        assert_eq!(split("/usr/sh", home), Some(Typed { dir_text: "/usr/".into(), dir: "/usr/".into(), name: "sh".into() }));
        assert_eq!(split("/", home).unwrap().name, "");
        assert_eq!(split("smb://nas/x", home), None);
        assert_eq!(split("~other/x", home), None, "another user's home is not looked up");
    }

    #[test]
    fn folders_and_archives_are_offered_folders_first() {
        let d = tempfile::tempdir().unwrap();
        for f in ["b-folder", "A-folder", ".hidden"] {
            std::fs::create_dir(d.path().join(f)).unwrap();
        }
        std::fs::write(d.path().join("notes.txt"), "").unwrap();
        std::fs::write(d.path().join("backup.zip"), "").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.path().join("b-folder"), d.path().join("link-to-b")).unwrap();
        let got = list(d.path(), |n| n.ends_with(".zip")).unwrap();
        let mut want = vec![(".hidden".to_string(), true), ("A-folder".into(), true), ("b-folder".into(), true)];
        #[cfg(unix)]
        want.push(("link-to-b".into(), true));
        want.push(("backup.zip".into(), false));
        assert_eq!(got, want);
        assert_eq!(texts("~/x/", &got[..2]), ["~/x/.hidden/", "~/x/A-folder/"]);
    }

    #[test]
    fn names_match_in_any_case_and_dot_names_only_for_a_dot() {
        assert!(matches("/home/me/Projects/", "/home/me/pro"));
        assert!(matches("/home/me/Projects/", "/home/me/"));
        assert!(!matches("/home/me/.cache/", "/home/me/"));
        assert!(matches("/home/me/.cache/", "/home/me/.c"));
        assert!(!matches("/home/me/Projects/", "/home/me/Projects/"), "typed in full already");
        assert!(!matches("/home/me/Projects/sub/", "/home/me/pro"), "only the folder being typed in");
        assert!(matches("~/Music/", "~"));
    }

    #[test]
    fn tab_goes_as_far_as_the_matches_agree() {
        let offers: Vec<String> = ["/p/Projects/", "/p/projects-old/", "/p/Pictures/", "/p/music.zip"].iter().map(|s| s.to_string()).collect();
        assert_eq!(tab("/p/pro", &offers).as_deref(), Some("/p/Projects"), "both project folders agree up to here");
        assert_eq!(tab("/p/Projects", &offers), None, "nothing more to add");
        assert_eq!(tab("/p/pi", &offers).as_deref(), Some("/p/Pictures/"), "one match is taken in full, its case too");
        assert_eq!(tab("/p/mu", &offers).as_deref(), Some("/p/music.zip"));
        assert_eq!(tab("/p/x", &offers), None);
    }
}
