//! Finding files by name below a folder, optionally inside archives.
//!
//! A query matches case-insensitively as a substring, or as a wildcard pattern when it
//! contains `*` or `?` (`*.jpg`, `IMG_20??_*`). Symbolic links are not followed (no loops,
//! no escaping the folder) and other filesystems mounted below are not entered.

use crate::archive::{Helper, is_browsable_name};
use crate::osprim;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone)]
pub struct Query {
    text: String,
    wildcard: bool,
}

impl Query {
    pub fn new(text: &str) -> Query {
        let text = text.trim().to_lowercase();
        let wildcard = text.contains('*') || text.contains('?');
        Query { text, wildcard }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn matches(&self, name: &str) -> bool {
        if self.text.is_empty() {
            return true;
        }
        let name = name.to_lowercase();
        if self.wildcard { wildcard_match(&self.text, &name) } else { name.contains(&self.text) }
    }
}

/// `*` any run of characters, `?` one character; the whole name must match.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    /// A file or folder on disk.
    File(PathBuf),
    /// An entry inside an archive file (entry path normalized, without trailing slash).
    InArchive { archive: PathBuf, entry: String, is_dir: bool, size: u64, mtime: Option<i64> },
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub hidden: bool,
    pub archives: bool,
}

/// Walks `root` and reports every match through `found`, as it is found. Returns false when
/// cancelled.
pub fn search(root: &Path, q: &Query, opts: Options, helper: Option<&Helper>, cancel: &AtomicBool, mut found: impl FnMut(Hit)) -> bool {
    let Ok(meta) = std::fs::metadata(root) else { return true };
    let dev = osprim::identity(root, &meta).dev;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        let mut subdirs = Vec::new();
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if !opts.hidden && name.starts_with('.') {
                continue;
            }
            let Ok(m) = e.metadata() else { continue };
            let path = e.path();
            if q.matches(&name) {
                found(Hit::File(path.clone()));
            }
            if m.is_dir() {
                if osprim::identity(&path, &m).dev == dev {
                    subdirs.push(path);
                }
            } else if opts.archives && m.is_file() && is_browsable_name(&name) {
                if let Some(h) = helper {
                    if let Ok(listing) = h.list(&path, None) {
                        for entry in listing.entries {
                            let leaf = entry.name.rsplit('/').next().unwrap_or(&entry.name);
                            if q.matches(leaf) && (opts.hidden || !entry.name.split('/').any(|p| p.starts_with('.'))) {
                                found(Hit::InArchive { archive: path.clone(), entry: entry.name.clone(), is_dir: entry.is_dir, size: entry.size, mtime: entry.mtime });
                            }
                        }
                    }
                }
            }
        }
        // Depth-first, in name order.
        subdirs.reverse();
        stack.extend(subdirs);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn substring_and_wildcards() {
        let q = Query::new("Photo");
        assert!(q.matches("my photo.JPG"));
        assert!(!q.matches("phot"));
        let w = Query::new("*.jpg");
        assert!(w.matches("a.JPG"));
        assert!(!w.matches("a.jpg.txt"));
        let w2 = Query::new("img_20??_*");
        assert!(w2.matches("IMG_2024_x.png"));
        assert!(!w2.matches("IMG_24_x.png"));
        assert!(Query::new("  ").is_empty());
    }

    #[test]
    fn walks_without_following_links_or_hidden_files() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        fs::create_dir_all(r.join("a/b")).unwrap();
        fs::create_dir_all(r.join(".hidden")).unwrap();
        fs::write(r.join("a/report.txt"), "").unwrap();
        fs::write(r.join("a/b/REPORT-2.txt"), "").unwrap();
        fs::write(r.join(".hidden/report-secret.txt"), "").unwrap();
        // the loop link proves links are not followed — a Unix-only fixture (creating links
        // on Windows needs Developer Mode; the rule itself is Unix-specific)
        #[cfg(unix)]
        std::os::unix::fs::symlink(r, r.join("a/loop")).unwrap();
        let q = Query::new("report");
        let mut hits = Vec::new();
        assert!(search(r, &q, Options::default(), None, &AtomicBool::new(false), |h| hits.push(h)));
        // A folder's own matches come before those of its subfolders.
        assert_eq!(hits, vec![Hit::File(r.join("a/report.txt")), Hit::File(r.join("a/b/REPORT-2.txt"))]);
        let mut all = Vec::new();
        search(r, &q, Options { hidden: true, archives: false }, None, &AtomicBool::new(false), |h| all.push(h));
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn cancelled_search_stops() {
        let d = tempfile::tempdir().unwrap();
        assert!(!search(d.path(), &Query::new("x"), Options::default(), None, &AtomicBool::new(true), |_| {}));
    }
}
