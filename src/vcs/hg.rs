//! Mercurial working copies, read through `hg` (rung 3: `hg help status`, `hg help templates`,
//! `hg help resolve`). Every answer is asked for in a shape the client documents: `status` with
//! `--print0`, the history and the annotation through templates whose fields are separated by the
//! unit mark and whose records end in the record mark, so a message with newlines of its own still
//! arrives as one revision.

use super::run;
use super::state::{count_unified, folder_status_from, BlameLine, Change, Commit, RefLabel, Refs, Status};
use super::Tool;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const US: char = '\u{1f}';
const RS: char = '\u{1e}';
/// The node Mercurial names a missing parent with.
const NULL: &str = "0000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hg {
    pub root: PathBuf,
}

impl Hg {
    fn text(&self, args: &[&str]) -> Result<String, String> {
        run::text("hg", &self.root, args)
    }

    /// `hg status` lines under `specs` (all of the working copy when empty): one change each, with
    /// the clean ones too when `all`.
    fn status(&self, specs: &[String], flags: &[&str]) -> Result<Vec<Change>, String> {
        let mut args = vec!["status", "--print0"];
        args.extend_from_slice(flags);
        args.push("--");
        let pats: Vec<String> = specs.iter().map(|p| pattern(p)).collect();
        args.extend(pats.iter().map(String::as_str));
        let conflicted = self.conflicted();
        Ok(parse_status(&self.text(&args)?)
            .into_iter()
            .map(|mut c| {
                if conflicted.contains(&c.path) {
                    c.status = Status::Conflicted;
                }
                c
            })
            .collect())
    }

    /// The files `hg resolve --list` still marks unresolved.
    fn conflicted(&self) -> Vec<String> {
        self.text(&["resolve", "--list", "--template", "{mergestatus} {path}\\0"]).map(|t| parse_resolve(&t)).unwrap_or_default()
    }
}

/// A path relative to the root as a pattern the client takes literally, wherever it runs from.
fn pattern(rel: &str) -> String {
    if rel.is_empty() { "path:.".to_string() } else { format!("path:{rel}") }
}

/// `hg status --print0`: "M path\0" per file.
pub fn parse_status(text: &str) -> Vec<Change> {
    text.split('\0')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            let (code, path) = rec.split_once(' ')?;
            let status = match code {
                "M" => Status::Modified,
                "A" => Status::Added,
                "R" | "!" => Status::Deleted,
                "C" => Status::Normal,
                "?" => Status::Unversioned,
                "I" => Status::Ignored,
                _ => return None,
            };
            Some(Change { path: path.to_string(), old_path: None, status, staged: false, added: None, removed: None })
        })
        .collect()
}

/// `hg resolve --list` with the template "{mergestatus} {path}\0" (resolve has no --print0): "U path\0" for what still waits, "R path\0" for what is resolved.
pub fn parse_resolve(text: &str) -> Vec<String> {
    text.split('\0').filter_map(|r| r.trim_start_matches('\n').strip_prefix("U ")).map(str::to_string).collect()
}

/// The template a revision is read with: node, parents, author's name, email, time, message.
fn log_template() -> String {
    format!("{{node}}{US}{{p1.node}} {{p2.node}}{US}{{author|person}}{US}{{author|email}}{US}{{date|hgdate}}{US}{{desc}}{RS}")
}

/// Revisions as the log template wrote them.
pub fn parse_log(text: &str) -> Vec<Commit> {
    text.split(RS)
        .filter(|r| !r.trim().is_empty())
        .filter_map(|r| {
            let f: Vec<&str> = r.trim_start_matches('\n').splitn(6, US).collect();
            let [node, parents, author, email, date, desc] = f.as_slice() else { return None };
            let time = date.split_whitespace().next()?.parse().ok()?;
            Some(Commit {
                hash: node.to_string(),
                parents: parents.split_whitespace().filter(|p| *p != NULL).map(str::to_string).collect(),
                author: author.to_string(),
                email: email.to_string(),
                time,
                committer: author.to_string(),
                commit_time: time,
                message: desc.to_string(),
            })
        })
        .collect()
}

/// `hg annotate` through a template: revision, author, time and the line itself, per line.
pub fn parse_annotate(text: &str) -> Vec<BlameLine> {
    text.split_inclusive('\n')
        .enumerate()
        .filter_map(|(i, rec)| {
            let f: Vec<&str> = rec.splitn(4, US).collect();
            let [node, user, date, line] = f.as_slice() else { return None };
            Some(BlameLine {
                hash: node.to_string(),
                author: user.to_string(),
                time: date.split_whitespace().next().and_then(|t| t.parse().ok()).unwrap_or(0),
                summary: String::new(),
                line: i + 1,
                text: line.strip_suffix('\n').unwrap_or(line).to_string(),
            })
        })
        .collect()
}

impl super::backend::Backend for Hg {
    fn tool(&self) -> Tool {
        Tool::Mercurial
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn branch_label(&self) -> String {
        self.text(&["branch"]).map(|s| s.trim().to_string()).unwrap_or_else(|_| "default".into())
    }

    fn changes(&self, paths: &[String], unversioned: bool, ignored: bool) -> Result<Vec<Change>, String> {
        let mut flags = vec!["--modified", "--added", "--removed", "--deleted"];
        if unversioned {
            flags.push("--unknown");
        }
        if ignored {
            flags.push("--ignored");
        }
        let mut list = self.status(paths, &flags)?;
        let pats: Vec<String> = paths.iter().map(|p| pattern(p)).collect();
        let mut args = vec!["diff", "--"];
        args.extend(pats.iter().map(String::as_str));
        let counts = count_unified(&self.text(&args).unwrap_or_default(), "b/");
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
        let all = self.status(&[rel.clone()], &["--all"])?;
        let tracked: Vec<String> = all.iter().filter(|c| !matches!(c.status, Status::Unversioned | Status::Ignored)).map(|c| c.path.clone()).collect();
        let changes: Vec<Change> = all.into_iter().filter(|c| c.status != Status::Normal).collect();
        Ok(folder_status_from(&rel, &changes, tracked.iter().map(String::as_str)))
    }

    fn summary(&self) -> Result<Status, String> {
        let list = self.status(&[], &["--modified", "--added", "--removed", "--deleted"])?;
        Ok(list.iter().map(|c| if c.status == Status::Conflicted { Status::Conflicted } else { Status::Modified }).max().unwrap_or(Status::Normal))
    }

    fn log(&self, paths: &[String], max: usize) -> Result<Vec<Commit>, String> {
        let limit = max.to_string();
        let template = log_template();
        let pats: Vec<String> = paths.iter().filter(|p| !p.is_empty()).map(|p| pattern(p)).collect();
        let mut args = vec!["log", "--limit", &limit, "--template", &template];
        if !pats.is_empty() {
            args.push("--");
            args.extend(pats.iter().map(String::as_str));
        }
        Ok(parse_log(&self.text(&args)?))
    }

    fn log_actions(&self, paths: &[String], max: usize) -> HashMap<String, Vec<char>> {
        let limit = max.to_string();
        let template = format!("{{node}}{US}{{if(file_adds,'A')}}{{if(file_mods,'M')}}{{if(file_dels,'D')}}{RS}");
        let pats: Vec<String> = paths.iter().filter(|p| !p.is_empty()).map(|p| pattern(p)).collect();
        let mut args = vec!["log", "--limit", &limit, "--template", &template];
        if !pats.is_empty() {
            args.push("--");
            args.extend(pats.iter().map(String::as_str));
        }
        self.text(&args)
            .unwrap_or_default()
            .split(RS)
            .filter_map(|r| r.trim_start_matches('\n').split_once(US))
            .map(|(node, kinds)| (node.to_string(), kinds.chars().collect()))
            .collect()
    }

    fn revision_changes(&self, rev: &str) -> Result<Vec<Change>, String> {
        let mut list = parse_status(&self.text(&["status", "--print0", "--change", rev])?);
        let counts = count_unified(&self.text(&["diff", "--change", rev]).unwrap_or_default(), "b/");
        for c in &mut list {
            if let Some(&(a, r)) = counts.get(&c.path) {
                (c.added, c.removed) = (a, r);
            }
        }
        Ok(list)
    }

    fn parent(&self, rev: &str) -> Option<String> {
        let p = self.text(&["log", "--rev", &format!("p1({rev})"), "--template", "{node}"]).ok()?;
        Some(p.trim().to_string()).filter(|p| !p.is_empty() && p != NULL)
    }

    fn blame(&self, path: &str) -> Result<Vec<BlameLine>, String> {
        let template = format!("{{lines % '{{node}}{US}{{user}}{US}{{date|hgdate}}{US}{{line}}'}}");
        let mut lines = parse_annotate(&self.text(&["annotate", "--template", &template, "--", &pattern(path)])?);
        let summaries: HashMap<String, String> = self.log(&[path.to_string()], 10_000).unwrap_or_default().into_iter().map(|c| (c.hash.clone(), c.subject().to_string())).collect();
        for l in &mut lines {
            l.summary = summaries.get(&l.hash).cloned().unwrap_or_default();
        }
        Ok(lines)
    }

    fn base_text(&self, path: &str) -> Vec<u8> {
        self.text_at(".", path)
    }

    fn text_at(&self, rev: &str, path: &str) -> Vec<u8> {
        if rev.is_empty() {
            return Vec::new();
        }
        run::output("hg", &self.root, &["cat", "--rev", rev, "--", &pattern(path)]).unwrap_or_default()
    }

    fn last_two_versions(&self, path: &str) -> Option<(String, String)> {
        let text = self.text(&["log", "--limit", "2", "--follow", "--template", "{node}\n", "--", &pattern(path)]).ok()?;
        let mut it = text.lines();
        let latest = it.next()?.to_string();
        Some((latest, it.next().unwrap_or("").to_string()))
    }

    fn has_conflicts(&self) -> bool {
        !self.conflicted().is_empty()
    }

    fn refs(&self) -> Refs {
        let mut r = Refs::default();
        for line in self.text(&["branches", "--template", &format!("{{branch}}{US}{{node}}\n")]).unwrap_or_default().lines() {
            if let Some((name, node)) = line.split_once(US) {
                r.branches.push(name.to_string());
                r.by_commit.entry(node.to_string()).or_default().push(RefLabel::Branch(name.to_string()));
            }
        }
        for line in self.text(&["tags", "--template", &format!("{{tag}}{US}{{node}}\n")]).unwrap_or_default().lines() {
            if let Some((name, node)) = line.split_once(US).filter(|(n, _)| *n != "tip") {
                r.tags.push(name.to_string());
                r.by_commit.entry(node.to_string()).or_default().push(RefLabel::Tag(name.to_string()));
            }
        }
        r
    }

    fn admin_dir(&self) -> Option<PathBuf> {
        Some(self.root.join(".hg"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_become_states() {
        let c = parse_status("M a.txt\0A b.txt\0R c.txt\0! d.txt\0C e.txt\0? f.txt\0I g.txt\0");
        let states: Vec<(&str, Status)> = c.iter().map(|c| (c.path.as_str(), c.status)).collect();
        assert_eq!(
            states,
            [("a.txt", Status::Modified), ("b.txt", Status::Added), ("c.txt", Status::Deleted), ("d.txt", Status::Deleted), ("e.txt", Status::Normal), ("f.txt", Status::Unversioned), ("g.txt", Status::Ignored)]
        );
    }

    #[test]
    fn a_name_with_spaces_stays_whole() {
        assert_eq!(parse_status("M my file.txt\0")[0].path, "my file.txt");
    }

    #[test]
    fn only_unresolved_files_are_conflicts() {
        assert_eq!(parse_resolve("U a.txt\0R b.txt\0"), ["a.txt"]);
    }

    #[test]
    fn a_revision_keeps_its_message_and_drops_the_null_parent() {
        let text = format!("abc{US}def {NULL}{US}Fix Ture{US}f@example{US}1700000000 0{US}second\n\nbody{RS}def{US}{NULL} {NULL}{US}Fix Ture{US}f@example{US}1690000000 -3600{US}first{RS}");
        let log = parse_log(&text);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].parents, ["def"]);
        assert_eq!(log[0].message, "second\n\nbody");
        assert_eq!(log[0].time, 1_700_000_000);
        assert!(log[1].parents.is_empty());
    }

    #[test]
    fn each_annotated_line_keeps_its_text_without_the_newline() {
        let text = format!("n1{US}fix{US}1700000000 0{US}alpha\nn2{US}fix{US}1700000100 0{US}beta\n");
        let lines = parse_annotate(&text);
        assert_eq!(lines.iter().map(|l| (l.hash.as_str(), l.line, l.text.as_str())).collect::<Vec<_>>(), [("n1", 1, "alpha"), ("n2", 2, "beta")]);
    }
}
