//! Subversion working copies, read through `svn` (rung 3: the Subversion book and the client's
//! own XML schemas, `subversion/svn/schema/{status,log,blame}.rnc`). Everything is asked for with
//! `--xml`, whose shape the client documents and keeps across versions; revisions are numbers,
//! named here as their decimal text.

use super::run;
use super::state::{count_unified, folder_status_from, BlameLine, Change, Commit, Status};
use super::xml;
use super::Tool;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Svn {
    pub root: PathBuf,
}

/// A path relative to the root as the client's argument: a trailing `@` keeps an `@` inside the
/// name from being read as a revision (the book's "peg revision" rule).
fn arg(rel: &str) -> String {
    let p = if rel.is_empty() { "." } else { rel };
    format!("{p}@")
}

/// The state an entry's `item` attribute names; a property change of an unchanged file is a
/// change too. A tree conflict outranks everything.
fn state_of(item: &str, props: Option<&str>, tree_conflicted: bool) -> Option<Status> {
    if tree_conflicted {
        return Some(Status::Conflicted);
    }
    Some(match item {
        "normal" | "none" if matches!(props, Some("modified") | Some("conflicted")) => if props == Some("conflicted") { Status::Conflicted } else { Status::Modified },
        "normal" => Status::Normal,
        "modified" | "merged" | "replaced" | "obstructed" | "incomplete" => Status::Modified,
        "added" => Status::Added,
        "deleted" | "missing" => Status::Deleted,
        "conflicted" => Status::Conflicted,
        "unversioned" => Status::Unversioned,
        "ignored" => Status::Ignored,
        // An external is a working copy of its own, and "none" says nothing about the file.
        _ => return None,
    })
}

/// `svn status --xml`: one change per entry, paths with '/' relative to where the client ran.
pub fn parse_status(text: &str) -> Vec<Change> {
    xml::elements(text, "entry")
        .filter_map(|e| {
            let path = e.attr("path")?.replace('\\', "/");
            let wc = e.children("wc-status").next()?;
            let status = state_of(&wc.attr("item")?, wc.attr("props").as_deref(), wc.attr("tree-conflicted").as_deref() == Some("true"))?;
            Some(Change { path: if path == "." { String::new() } else { path }, old_path: None, status, staged: false, added: None, removed: None })
        })
        .collect()
}

/// A time as the client writes it ("2026-11-14T02:33:19.716000Z") in seconds since 1970.
pub fn epoch(iso: &str) -> i64 {
    gtk::glib::DateTime::from_iso8601(iso.trim(), Some(&gtk::glib::TimeZone::utc())).map(|d| d.to_unix()).unwrap_or(0)
}

/// One revision of `svn log --xml --verbose`, with the paths it touched (repository paths) and
/// their action letters.
pub struct LogEntry {
    pub commit: Commit,
    pub paths: Vec<(char, String)>,
}

pub fn parse_log(text: &str) -> Vec<LogEntry> {
    let mut out: Vec<LogEntry> = xml::elements(text, "logentry")
        .filter_map(|e| {
            let rev = e.attr("revision")?;
            let author = e.child_text("author").unwrap_or_default();
            let time = e.child_text("date").map(|d| epoch(&d)).unwrap_or(0);
            let paths = e
                .children("paths")
                .next()
                .map(|p| p.children("path").filter_map(|p| Some((p.attr("action")?.chars().next()?, xml::decode(p.body.trim())))).collect())
                .unwrap_or_default();
            Some(LogEntry {
                commit: Commit { hash: rev, parents: Vec::new(), author: author.clone(), email: String::new(), time, committer: author, commit_time: time, message: e.child_text("msg").unwrap_or_default() },
                paths,
            })
        })
        .collect();
    // The history is one line: each revision's parent is the next older one listed.
    for i in 0..out.len() {
        if let Some(older) = out.get(i + 1).map(|o| o.commit.hash.clone()) {
            out[i].commit.parents = vec![older];
        }
    }
    out
}

/// `svn blame --xml`: per line its number and the revision, author and time that last changed it;
/// the text comes from the file itself, which the annotation leaves out.
pub fn parse_blame(text: &str, content: &str) -> Vec<BlameLine> {
    let lines: Vec<&str> = content.lines().collect();
    xml::elements(text, "entry")
        .filter_map(|e| {
            let line: usize = e.attr("line-number")?.parse().ok()?;
            let c = e.children("commit").next();
            Some(BlameLine {
                hash: c.and_then(|c| c.attr("revision")).unwrap_or_default(),
                author: c.and_then(|c| c.child_text("author")).unwrap_or_default(),
                time: c.and_then(|c| c.child_text("date")).map(|d| epoch(&d)).unwrap_or(0),
                summary: String::new(),
                line,
                text: lines.get(line - 1).copied().unwrap_or("").to_string(),
            })
        })
        .collect()
}

impl Svn {
    fn text(&self, args: &[&str]) -> Result<String, String> {
        run::text("svn", &self.root, args)
    }

    fn status(&self, specs: &[String], flags: &[&str]) -> Result<Vec<Change>, String> {
        let mut args = vec!["status", "--xml"];
        args.extend_from_slice(flags);
        let targets: Vec<String> = if specs.is_empty() { vec![arg("")] } else { specs.iter().map(|s| arg(s)).collect() };
        args.push("--");
        args.extend(targets.iter().map(String::as_str));
        Ok(parse_status(&self.text(&args)?))
    }

    /// The working copy's place in its repository ("/trunk"), from which the log's repository
    /// paths are made relative.
    fn repository_path(&self) -> String {
        self.text(&["info", "--show-item", "relative-url", "--", "."]).map(|s| s.trim().trim_start_matches('^').to_string()).unwrap_or_default()
    }

    /// The history up to the repository's newest revision: a working copy's own revision stays
    /// where its last update left it, so a commit made since would be missing. A path the newest
    /// revision does not hold yet (added, not committed) falls back to the working copy's own.
    fn newest_first(&self, limit: &str, paths: &[String]) -> Result<Vec<LogEntry>, String> {
        self.log_entries(&["--revision", "HEAD:1", "--limit", limit], paths).or_else(|_| self.log_entries(&["--limit", limit], paths))
    }

    fn log_entries(&self, extra: &[&str], paths: &[String]) -> Result<Vec<LogEntry>, String> {
        let mut args = vec!["log", "--xml", "--verbose"];
        args.extend_from_slice(extra);
        let targets: Vec<String> = paths.iter().filter(|p| !p.is_empty()).map(|p| arg(p)).collect();
        args.push("--");
        if targets.is_empty() {
            args.push(".");
        }
        args.extend(targets.iter().map(String::as_str));
        Ok(parse_log(&self.text(&args)?))
    }
}

impl super::backend::Backend for Svn {
    fn tool(&self) -> Tool {
        Tool::Subversion
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn branch_label(&self) -> String {
        self.text(&["info", "--show-item", "relative-url", "--", "."]).map(|s| s.trim().to_string()).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "^/".into())
    }

    fn changes(&self, paths: &[String], unversioned: bool, ignored: bool) -> Result<Vec<Change>, String> {
        let flags: &[&str] = if ignored { &["--no-ignore"] } else { &[] };
        let mut list: Vec<Change> = self
            .status(paths, flags)?
            .into_iter()
            .filter(|c| !c.path.is_empty() && c.status != Status::Normal)
            .filter(|c| unversioned || c.status != Status::Unversioned)
            .filter(|c| ignored || c.status != Status::Ignored)
            .collect();
        let targets: Vec<String> = paths.iter().map(|p| arg(p)).collect();
        let mut args = vec!["diff", "--"];
        args.extend(targets.iter().map(String::as_str));
        let counts = count_unified(&self.text(&args).unwrap_or_default(), "");
        for c in &mut list {
            if let Some(&(a, r)) = counts.get(&c.path) {
                (c.added, c.removed) = (a, r);
            } else if c.status == Status::Unversioned {
                (c.added, c.removed) = super::state::new_file_lines(&self.root.join(&c.path));
            }
        }
        list.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(list)
    }

    fn folder_status(&self, dir: &Path) -> Result<HashMap<OsString, Status>, String> {
        let rel = self.relative(dir).ok_or("not in this working copy")?;
        let all = self.status(&[rel.clone()], &["--verbose", "--no-ignore"])?;
        let tracked: Vec<String> = all.iter().filter(|c| !c.path.is_empty() && !matches!(c.status, Status::Unversioned | Status::Ignored)).map(|c| c.path.clone()).collect();
        let changes: Vec<Change> = all.into_iter().filter(|c| !c.path.is_empty() && c.status != Status::Normal).collect();
        Ok(folder_status_from(&rel, &changes, tracked.iter().map(String::as_str)))
    }

    fn summary(&self) -> Result<Status, String> {
        let list = self.status(&[], &["--quiet"])?;
        Ok(list.iter().filter(|c| c.status.is_change()).map(|c| if c.status == Status::Conflicted { Status::Conflicted } else { Status::Modified }).max().unwrap_or(Status::Normal))
    }

    fn log(&self, paths: &[String], max: usize) -> Result<Vec<Commit>, String> {
        let limit = max.to_string();
        Ok(self.newest_first(&limit, paths)?.into_iter().map(|e| e.commit).collect())
    }

    fn log_actions(&self, paths: &[String], max: usize) -> HashMap<String, Vec<char>> {
        let limit = max.to_string();
        self.newest_first(&limit, paths)
            .unwrap_or_default()
            .into_iter()
            .map(|e| {
                let mut kinds: Vec<char> = e.paths.iter().map(|(a, _)| if *a == 'R' { 'M' } else { *a }).collect();
                kinds.sort_by_key(|k| "AMDR".find(*k));
                kinds.dedup();
                (e.commit.hash, kinds)
            })
            .collect()
    }

    fn revision_changes(&self, rev: &str) -> Result<Vec<Change>, String> {
        let base = format!("{}/", self.repository_path());
        let entries = self.log_entries(&["--revision", rev], &[])?;
        let counts = count_unified(&self.text(&["diff", "--change", rev, "--", "."]).unwrap_or_default(), "");
        Ok(entries
            .into_iter()
            .flat_map(|e| e.paths)
            .filter_map(|(action, path)| {
                let rel = path.strip_prefix(&base)?.to_string();
                let status = match action {
                    'A' => Status::Added,
                    'D' => Status::Deleted,
                    _ => Status::Modified,
                };
                let (added, removed) = counts.get(&rel).copied().unwrap_or((None, None));
                Some(Change { path: rel, old_path: None, status, staged: false, added, removed })
            })
            .collect())
    }

    fn parent(&self, rev: &str) -> Option<String> {
        rev.parse::<u64>().ok().filter(|r| *r > 0).map(|r| (r - 1).to_string())
    }

    fn blame(&self, path: &str) -> Result<Vec<BlameLine>, String> {
        let xml = self.text(&["blame", "--xml", "--", &arg(path)])?;
        let content = String::from_utf8_lossy(&self.base_text(path)).into_owned();
        let mut lines = parse_blame(&xml, &content);
        let summaries: HashMap<String, String> = self.log(&[path.to_string()], 10_000).unwrap_or_default().into_iter().map(|c| (c.hash.clone(), c.subject().to_string())).collect();
        for l in &mut lines {
            l.summary = summaries.get(&l.hash).cloned().unwrap_or_default();
        }
        Ok(lines)
    }

    fn base_text(&self, path: &str) -> Vec<u8> {
        self.text_at("BASE", path)
    }

    fn text_at(&self, rev: &str, path: &str) -> Vec<u8> {
        if rev.is_empty() {
            return Vec::new();
        }
        run::output("svn", &self.root, &["cat", "--revision", rev, "--", &arg(path)]).unwrap_or_default()
    }

    fn last_two_versions(&self, path: &str) -> Option<(String, String)> {
        let entries = self.log_entries(&["--limit", "2"], &[path.to_string()]).ok()?;
        let mut it = entries.into_iter().map(|e| e.commit.hash);
        let latest = it.next()?;
        Some((latest, it.next().unwrap_or_default()))
    }

    fn has_conflicts(&self) -> bool {
        self.status(&[], &["--quiet"]).is_ok_and(|l| l.iter().any(|c| c.status == Status::Conflicted))
    }

    fn admin_dir(&self) -> Option<PathBuf> {
        Some(self.root.join(".svn"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<status>
<target
   path=".">
<entry
   path=".">
<wc-status
   props="none"
   item="normal"
   revision="3">
</wc-status>
</entry>
<entry
   path="one.txt">
<wc-status
   props="none"
   item="modified"
   revision="3">
<commit
   revision="3">
<author>fixture</author>
<date>2026-11-14T02:33:19.716000Z</date>
</commit>
</wc-status>
</entry>
<entry
   path="sub\new.txt">
<wc-status
   props="none"
   item="added"
   revision="-1">
</wc-status>
</entry>
<entry
   path="tc.txt">
<wc-status
   props="none"
   item="normal"
   tree-conflicted="true"
   revision="3">
</wc-status>
</entry>
<entry
   path="loose.txt">
<wc-status
   props="none"
   item="unversioned">
</wc-status>
</entry>
</target>
</status>"#;

    #[test]
    fn status_entries_become_states_with_forward_slashes() {
        let c = parse_status(STATUS);
        let got: Vec<(&str, Status)> = c.iter().map(|c| (c.path.as_str(), c.status)).collect();
        assert_eq!(got, [("", Status::Normal), ("one.txt", Status::Modified), ("sub/new.txt", Status::Added), ("tc.txt", Status::Conflicted), ("loose.txt", Status::Unversioned)]);
    }

    #[test]
    fn a_property_change_alone_is_a_change() {
        assert_eq!(state_of("normal", Some("modified"), false), Some(Status::Modified));
        assert_eq!(state_of("external", None, false), None);
    }

    #[test]
    fn the_log_is_one_line_of_revisions() {
        let x = r#"<log><logentry revision="3"><author>fixture</author><date>2026-11-14T02:33:19.716000Z</date><paths><path action="M" kind="file">/trunk/one.txt</path></paths><msg>second</msg></logentry><logentry revision="2"><author>fixture</author><date>2026-11-14T02:33:18Z</date><paths><path action="A" kind="file">/trunk/one.txt</path></paths><msg>first</msg></logentry></log>"#;
        let log = parse_log(x);
        assert_eq!(log[0].commit.hash, "3");
        assert_eq!(log[0].commit.parents, ["2"]);
        assert!(log[1].commit.parents.is_empty());
        assert_eq!(log[0].paths, [('M', "/trunk/one.txt".to_string())]);
        assert_eq!(log[0].commit.time, 1_794_623_599);
    }

    #[test]
    fn blame_takes_its_text_from_the_file() {
        let x = r#"<blame><target path="one.txt"><entry line-number="1"><commit revision="2"><author>a</author><date>2026-11-14T02:33:18Z</date></commit></entry><entry line-number="2"><commit revision="3"><author>b</author><date>2026-11-14T02:33:19Z</date></commit></entry></target></blame>"#;
        let b = parse_blame(x, "alpha\nbeta\n");
        assert_eq!(b.iter().map(|l| (l.hash.as_str(), l.author.as_str(), l.text.as_str())).collect::<Vec<_>>(), [("2", "a", "alpha"), ("3", "b", "beta")]);
    }
}
