//! Visual Source Safe bindings, read through `ss` (Windows only). The shapes parsed here are the
//! ones Microsoft's command-line reference shows for `ss Status`, `ss Dir`, `ss History` and
//! `ss Project`; no `ss` runs on the machines this is built and tested on, so they are pinned
//! against recorded output and remain a hypothesis until measured (README: Version control).
//!
//! VSS keeps its binding folder by folder and works on whole files: a file checked out (to anyone)
//! is the change it knows of, a file of the project not checked out is unchanged, and a file the
//! project does not list is unversioned. It records no parents, no blame and no merges.

use super::run;
use super::state::{folder_status_from, BlameLine, Change, Commit, Status};
use super::Tool;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vss {
    pub root: PathBuf,
}

/// `ss Status`: the checked-out files, one per line: name, user, check-out kind, date, time, the
/// folder it is checked out to. Lines that are no file (the project header, "No checked out
/// files found.") are skipped.
pub fn parse_status(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| !l.starts_with('$') && !l.trim().is_empty() && !l.starts_with("No checked out"))
        .filter_map(|l| {
            // The name column is padded to the user column; a name with spaces keeps them, so the
            // line is cut at the run of blanks before the user.
            let cut = l.find("  ")?;
            Some(l[..cut].trim().to_string())
        })
        .collect()
}

/// `ss Dir`: the project's own listing — "$/proj:", then file names, "$sub" for subprojects and a
/// closing "N item(s)" count.
pub fn parse_dir(text: &str) -> (Vec<String>, Vec<String>) {
    let mut files = Vec::new();
    let mut projects = Vec::new();
    for l in text.lines().map(str::trim) {
        if l.is_empty() || l.ends_with(':') || l.ends_with("item(s)") || l.starts_with("No items found") {
            continue;
        }
        match l.strip_prefix('$') {
            Some(sub) => projects.push(sub.to_string()),
            None => files.push(l.to_string()),
        }
    }
    (files, projects)
}

/// `ss History`: "*****  Version N  *****" blocks with "User: … Date: … Time: …", the action
/// ("Checked in $/proj") and "Comment:" lines.
pub fn parse_history(text: &str) -> Vec<Commit> {
    let mut out: Vec<Commit> = Vec::new();
    for block in text.split("*****************  Version ").skip(1) {
        let Some(version) = block.split_whitespace().next() else { continue };
        let mut author = String::new();
        let mut time = 0;
        let mut comment: Vec<String> = Vec::new();
        let mut in_comment = false;
        for l in block.lines().skip(1) {
            if let Some(rest) = l.strip_prefix("User: ") {
                let mut parts = rest.split_whitespace();
                author = parts.next().unwrap_or("").to_string();
                let date = rest.split("Date:").nth(1).and_then(|d| d.split_whitespace().next()).unwrap_or("");
                let clock = rest.split("Time:").nth(1).map(str::trim).unwrap_or("");
                time = history_time(date, clock);
            } else if let Some(c) = l.strip_prefix("Comment: ") {
                in_comment = true;
                comment.push(c.trim_end().to_string());
            } else if in_comment && !l.trim().is_empty() {
                comment.push(l.trim_end().to_string());
            }
        }
        out.push(Commit { hash: version.to_string(), parents: Vec::new(), author: author.clone(), email: String::new(), time, committer: author, commit_time: time, message: comment.join("\n") });
    }
    for i in 0..out.len() {
        if let Some(older) = out.get(i + 1).map(|o| o.hash.clone()) {
            out[i].parents = vec![older];
        }
    }
    out
}

/// "11/14/26" and " 2:33a" (US short date, 12-hour clock with a/p) as seconds; VSS writes local
/// time without a zone, which is taken as UTC here.
fn history_time(date: &str, clock: &str) -> i64 {
    let d: Vec<i64> = date.split('/').filter_map(|p| p.parse().ok()).collect();
    let [m, day, y] = d.as_slice() else { return 0 };
    let year = if *y < 100 { if *y < 70 { 2000 + y } else { 1900 + y } } else { *y };
    let pm = clock.ends_with('p');
    let hm: Vec<i64> = clock.trim_end_matches(['a', 'p']).split(':').filter_map(|p| p.trim().parse().ok()).collect();
    let (mut h, mi) = (hm.first().copied().unwrap_or(0), hm.get(1).copied().unwrap_or(0));
    if h == 12 {
        h = 0;
    }
    if pm {
        h += 12;
    }
    let days = gtk::glib::DateTime::from_utc(year as i32, *m as i32, *day as i32, 0, 0, 0.0).map(|t| t.to_unix() / 86_400).unwrap_or(0);
    days * 86_400 + h * 3600 + mi * 60
}

impl Vss {
    fn text(&self, args: &[&str]) -> Result<String, String> {
        run::text("ss", &self.root, args)
    }

    fn checked_out(&self) -> Vec<String> {
        self.text(&["Status", "-I-"]).map(|t| parse_status(&t)).unwrap_or_default()
    }
}

impl super::backend::Backend for Vss {
    fn tool(&self) -> Tool {
        Tool::VisualSourceSafe
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn branch_label(&self) -> String {
        // "Current project is $/proj"
        self.text(&["Project"]).ok().and_then(|t| t.split_whitespace().last().map(str::to_string)).unwrap_or_else(|| "$/".into())
    }

    fn changes(&self, paths: &[String], unversioned: bool, _ignored: bool) -> Result<Vec<Change>, String> {
        let mut out: Vec<Change> = self.checked_out().into_iter().map(|p| Change { path: p, old_path: None, status: Status::Modified, staged: false, added: None, removed: None }).collect();
        if unversioned {
            let st = self.folder_status(&self.root)?;
            out.extend(st.into_iter().filter(|(_, s)| *s == Status::Unversioned).map(|(n, _)| Change { path: n.to_string_lossy().into_owned(), old_path: None, status: Status::Unversioned, staged: false, added: None, removed: None }));
        }
        if !paths.is_empty() && !paths.iter().any(String::is_empty) {
            out.retain(|c| paths.iter().any(|p| c.path == *p || c.path.starts_with(&format!("{p}/"))));
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    fn folder_status(&self, dir: &Path) -> Result<HashMap<OsString, Status>, String> {
        let rel = self.relative(dir).ok_or("not in this binding")?;
        let (files, projects) = parse_dir(&self.text(&["Dir", "-I-"])?);
        let checked = self.checked_out();
        let mut tracked: Vec<String> = files.clone();
        tracked.extend(projects);
        let mut changes: Vec<Change> = checked.iter().map(|p| Change { path: p.clone(), old_path: None, status: Status::Modified, staged: false, added: None, removed: None }).collect();
        if let Ok(read) = std::fs::read_dir(dir) {
            for e in read.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.eq_ignore_ascii_case("vssver.scc") || name.eq_ignore_ascii_case("vssver2.scc") || tracked.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
                    continue;
                }
                changes.push(Change { path: name, old_path: None, status: Status::Unversioned, staged: false, added: None, removed: None });
            }
        }
        Ok(folder_status_from(&rel, &changes, tracked.iter().map(String::as_str)))
    }

    fn summary(&self) -> Result<Status, String> {
        Ok(if self.checked_out().is_empty() { Status::Normal } else { Status::Modified })
    }

    fn log(&self, paths: &[String], max: usize) -> Result<Vec<Commit>, String> {
        let mut args = vec!["History", "-I-"];
        args.extend(paths.iter().filter(|p| !p.is_empty()).map(String::as_str));
        let mut log = parse_history(&self.text(&args)?);
        log.truncate(max);
        Ok(log)
    }

    fn revision_changes(&self, _rev: &str) -> Result<Vec<Change>, String> {
        Ok(Vec::new())
    }

    fn parent(&self, rev: &str) -> Option<String> {
        rev.parse::<u64>().ok().filter(|r| *r > 1).map(|r| (r - 1).to_string())
    }

    fn blame(&self, _path: &str) -> Result<Vec<BlameLine>, String> {
        Err("Visual Source Safe keeps no blame.".into())
    }

    fn base_text(&self, path: &str) -> Vec<u8> {
        run::output("ss", &self.root, &["View", "-I-", path]).unwrap_or_default()
    }

    fn text_at(&self, rev: &str, path: &str) -> Vec<u8> {
        if rev.is_empty() {
            return Vec::new();
        }
        run::output("ss", &self.root, &["View", "-I-", &format!("-V{rev}"), path]).unwrap_or_default()
    }

    fn last_two_versions(&self, path: &str) -> Option<(String, String)> {
        let log = self.log(&[path.to_string()], 2).ok()?;
        let mut it = log.into_iter().map(|c| c.hash);
        Some((it.next()?, it.next().unwrap_or_default()))
    }

    fn has_conflicts(&self) -> bool {
        false
    }

    fn admin_dir(&self) -> Option<PathBuf> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_out_files_are_listed_by_name() {
        let t = "$/proj:\na.txt                   Hawkynt  Exc   11/14/26  2:33a  C:\\work\\proj\nmy notes.txt            Other    Exc   11/13/26 11:02p  C:\\elsewhere\n";
        assert_eq!(parse_status(t), ["a.txt", "my notes.txt"]);
        assert!(parse_status("No checked out files found.\n").is_empty());
    }

    #[test]
    fn a_project_lists_its_files_and_subprojects() {
        let (files, projects) = parse_dir("$/proj:\n$sub\na.txt\nb.txt\n\n3 item(s)\n");
        assert_eq!(files, ["a.txt", "b.txt"]);
        assert_eq!(projects, ["sub"]);
    }

    #[test]
    fn history_blocks_become_revisions_newest_first() {
        let t = "History of $/proj/a.txt ...\n\n*****************  Version 2   *****************\nUser: Hawkynt      Date: 11/14/26   Time:  2:33p\nChecked in $/proj\nComment: second\n  more\n\n*****************  Version 1   *****************\nUser: Hawkynt      Date: 11/14/26   Time: 12:05a\nCreated\nComment: first\n";
        let h = parse_history(t);
        assert_eq!(h.iter().map(|c| (c.hash.as_str(), c.message.as_str())).collect::<Vec<_>>(), [("2", "second\n  more"), ("1", "first")]);
        assert_eq!(h[0].parents, ["1"]);
        assert_eq!(h[0].time - h[1].time, 14 * 3600 + 28 * 60);
    }
}
