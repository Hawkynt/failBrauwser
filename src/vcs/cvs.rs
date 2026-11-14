//! CVS working copies. CVS asks its server for every `cvs status`, so the states of the overlays
//! are read from what CVS itself keeps in the working copy, as TortoiseCVS does (rung 3: the CVS
//! manual, "Working directory storage" and "Ignoring files"): each folder's `CVS/Entries` (amended
//! by `CVS/Entries.Log`), its `CVS/Tag`, `.cvsignore` and CVS's default ignore list. History,
//! annotation and old texts come from the client: `cvs log`, `cvs annotate`, `cvs update -p`.
//!
//! CVS records no changesets, only file revisions; the log groups the revisions that share author
//! and message and lie within five minutes of each other into one entry, as `cvsps` does, and names
//! that entry by the time of its newest revision (seconds since 1970). A file revision ("1.2") is a
//! revision too, wherever a single file is concerned.

use super::state::{count_unified, folder_status_from, BlameLine, Change, Commit, Status};
use super::Tool;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// How far apart the revisions of one changeset may lie, as `cvsps` takes it.
const FUZZ: i64 = 300;

/// CVS's own default ignore list (the manual's "Ignoring files").
const DEFAULT_IGNORE: &[&str] = &[
    "RCS", "SCCS", "CVS", "CVS.adm", "RCSLOG", "cvslog.*", "tags", "TAGS", ".make.state", ".nse_depinfo", "*~", "#*", ".#*", ",*", "_$*", "*$", "*.old", "*.bak", "*.BAK", "*.orig", "*.rej", ".del-*", "*.a",
    "*.olb", "*.o", "*.obj", "*.so", "*.exe", "*.Z", "*.elc", "*.ln", "core",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cvs {
    pub root: PathBuf,
}

/// One line of `CVS/Entries`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    File { name: String, revision: String, timestamp: String, conflict: bool },
    Dir { name: String },
}

/// `CVS/Entries`, with the additions and removals `CVS/Entries.Log` records applied.
pub fn parse_entries(entries: &str, log: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = entries.lines().filter_map(parse_entry).collect();
    for line in log.lines() {
        let (op, rest) = line.split_at(line.len().min(2));
        let Some(e) = parse_entry(rest) else { continue };
        let name = entry_name(&e).to_string();
        out.retain(|x| entry_name(x) != name);
        if op == "A " {
            out.push(e);
        }
    }
    out
}

fn entry_name(e: &Entry) -> &str {
    match e {
        Entry::File { name, .. } | Entry::Dir { name } => name,
    }
}

fn parse_entry(line: &str) -> Option<Entry> {
    if let Some(rest) = line.strip_prefix("D/") {
        return Some(Entry::Dir { name: rest.split('/').next()?.to_string() });
    }
    let f: Vec<&str> = line.strip_prefix('/')?.split('/').collect();
    let (name, revision, stamp) = (*f.first()?, *f.get(1)?, *f.get(2)?);
    let (timestamp, conflict) = match stamp.split_once('+') {
        Some((t, c)) => (t.to_string(), !c.is_empty() || t == "Result of merge"),
        None => (stamp.to_string(), false),
    };
    Some(Entry::File { name: name.to_string(), revision: revision.to_string(), timestamp, conflict })
}

/// A time as `asctime` writes it in UTC ("Sun Apr  7 01:29:26 1996"): the form `CVS/Entries`
/// records a file's modification time in, compared as text the way CVS compares it.
pub fn asctime(epoch: i64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    format!("{} {} {:>2} {:02}:{:02}:{:02} {}", DAYS[days.rem_euclid(7) as usize], MONTHS[(m - 1) as usize], d, secs / 3600, secs / 60 % 60, secs % 60, y)
}

/// The proleptic Gregorian date of a day count since 1970 (Howard Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The day count of a date (the inverse of `civil`).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// "2026/11/14 02:33:19" (CVS 1.11) or "2026-11-14 02:33:19 +0000" (1.12), UTC, in seconds.
pub fn log_time(s: &str) -> i64 {
    let s = s.trim();
    let mut it = s.split(|c: char| c == '/' || c == '-' || c == ' ' || c == ':').filter(|p| !p.is_empty());
    let mut n = || it.next().and_then(|p| p.parse::<i64>().ok()).unwrap_or(0);
    let (y, mo, d, h, mi, se) = (n(), n(), n(), n(), n(), n());
    days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3600 + mi * 60 + se
}

/// Whether `name` matches a `.cvsignore` pattern (sh wildcards: `*`, `?`, `[…]`).
pub fn ignored_by(patterns: &[String], name: &str) -> bool {
    patterns.iter().any(|p| glob(p.as_bytes(), name.as_bytes()))
}

fn glob(p: &[u8], s: &[u8]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => glob(&p[1..], s) || (!s.is_empty() && glob(p, &s[1..])),
        (Some(b'?'), Some(_)) => glob(&p[1..], &s[1..]),
        (Some(b'['), Some(&c)) => match p.iter().position(|&b| b == b']').filter(|&e| e > 1) {
            Some(end) => {
                let set = &p[1..end];
                let (negate, set) = if set.first() == Some(&b'!') { (true, &set[1..]) } else { (false, set) };
                let mut hit = false;
                let mut i = 0;
                while i < set.len() {
                    if i + 2 < set.len() && set[i + 1] == b'-' {
                        hit |= set[i] <= c && c <= set[i + 2];
                        i += 3;
                    } else {
                        hit |= set[i] == c;
                        i += 1;
                    }
                }
                hit != negate && glob(&p[end + 1..], &s[1..])
            }
            None => c == b'[' && glob(&p[1..], &s[1..]),
        },
        (Some(a), Some(b)) => a == b && glob(&p[1..], &s[1..]),
        _ => false,
    }
}

/// One file revision of `cvs log`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRevision {
    pub path: String,
    pub revision: String,
    pub time: i64,
    pub author: String,
    pub dead: bool,
    pub added: Option<u64>,
    pub removed: Option<u64>,
    pub message: String,
}

/// `cvs log` (the rlog format): the revisions of every file it names.
pub fn parse_log(text: &str) -> Vec<FileRevision> {
    let mut out = Vec::new();
    for file in text.split("\n=============================================================================") {
        let Some(path) = file.lines().find_map(|l| l.strip_prefix("Working file: ")).map(str::to_string) else { continue };
        for block in file.split("\n----------------------------\n").skip(1) {
            let mut lines = block.lines();
            let Some(revision) = lines.next().and_then(|l| l.strip_prefix("revision ")).map(|r| r.split_whitespace().next().unwrap_or(r).to_string()) else { continue };
            let Some(meta) = lines.next() else { continue };
            let field = |name: &str| meta.split(';').find_map(|f| f.trim().strip_prefix(name)).map(|v| v.trim().to_string());
            let lines_field = field("lines:");
            let count = |sign: char| lines_field.as_deref().and_then(|l| l.split_whitespace().find_map(|p| p.strip_prefix(sign))).and_then(|n| n.parse().ok());
            let message: Vec<&str> = lines.skip_while(|l| l.starts_with("branches:")).collect();
            out.push(FileRevision {
                path: path.clone(),
                time: field("date:").map(|d| log_time(&d)).unwrap_or(0),
                author: field("author:").unwrap_or_default(),
                dead: field("state:").as_deref() == Some("dead"),
                added: count('+'),
                removed: count('-'),
                message: message.join("\n").trim_end().to_string(),
                revision,
            });
        }
    }
    out
}

/// A changeset: the file revisions that share author and message within `FUZZ` seconds, newest
/// first, named by the time of its newest revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changeset {
    pub id: String,
    pub revisions: Vec<FileRevision>,
}

pub fn changesets(mut revisions: Vec<FileRevision>) -> Vec<Changeset> {
    // The vendor branch an import leaves (1.1.1.x) and its "Initial revision" twin are one import.
    revisions.sort_by(|a, b| b.time.cmp(&a.time).then(a.path.cmp(&b.path)));
    let mut out: Vec<Changeset> = Vec::new();
    for r in revisions {
        let joins = out.iter_mut().rev().take(8).find(|c| {
            let first = &c.revisions[0];
            first.author == r.author && first.message == r.message && first.time - r.time <= FUZZ && !c.revisions.iter().any(|x| x.path == r.path)
        });
        match joins {
            Some(c) => c.revisions.push(r),
            None => out.push(Changeset { id: r.time.to_string(), revisions: vec![r] }),
        }
    }
    out
}

/// `cvs annotate`: "1.2          (author   14-Nov-26): text" per line.
pub fn parse_annotate(text: &str) -> Vec<BlameLine> {
    text.lines()
        .filter_map(|l| {
            let (head, rest) = l.split_once(" (")?;
            let (meta, line) = rest.split_once("): ").or_else(|| rest.strip_suffix("):").map(|m| (m, "")))?;
            let mut m = meta.split_whitespace();
            let author = m.next()?.to_string();
            let date = m.next().unwrap_or("");
            Some((head.trim().to_string(), author, annotate_date(date), line.to_string()))
        })
        .enumerate()
        .map(|(i, (hash, author, time, text))| BlameLine { hash, author, time, summary: String::new(), line: i + 1, text })
        .collect()
}

/// "14-Nov-26" as midnight UTC of that day.
fn annotate_date(d: &str) -> i64 {
    let mut it = d.split('-');
    let day: u32 = it.next().and_then(|x| x.parse().ok()).unwrap_or(1);
    let month = it.next().and_then(|m| ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].iter().position(|x| *x == m)).map(|i| i as u32 + 1).unwrap_or(1);
    let yy: i64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(70);
    let year = if yy < 70 { 2000 + yy } else { 1900 + yy };
    days_from_civil(year, month, day) * 86_400
}

/// The revision before a file revision ("1.2" → "1.1"; the first of a branch → where it branched).
pub fn previous_revision(rev: &str) -> Option<String> {
    let mut parts: Vec<u64> = rev.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let last = parts.pop()?;
    if last > 1 {
        parts.push(last - 1);
    } else if parts.len() >= 3 {
        parts.pop(); // 1.2.2.1 → 1.2
    } else {
        return None;
    }
    Some(parts.iter().map(u64::to_string).collect::<Vec<_>>().join("."))
}

/// A changeset's name (seconds since 1970) as the date `-D` takes: "2026-11-14 02:33:19 UTC".
pub fn date_arg(id: &str) -> String {
    let secs: i64 = id.parse().unwrap_or(0);
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", s / 3600, s / 60 % 60, s % 60)
}

impl Cvs {
    /// A client call; CVS answers 1 for "there were differences", which is no failure here.
    fn client(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let out = Command::new("cvs").current_dir(&self.root).args(args).env("LC_ALL", "C").env("CVS_RSH", "ssh").stdin(Stdio::null()).output().map_err(|e| format!("cvs could not be started: {e}"))?;
        match out.status.code() {
            Some(0) | Some(1) => Ok(out.stdout),
            _ => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        }
    }

    fn text(&self, args: &[&str]) -> Result<String, String> {
        self.client(args).map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// The `.cvsignore` patterns that apply in `dir`: CVS's defaults, `$CVSIGNORE`, the user's
    /// `~/.cvsignore` and the folder's own file; a `!` clears what came before.
    fn ignore_patterns(&self, dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = DEFAULT_IGNORE.iter().map(|s| s.to_string()).collect();
        let mut add = |text: &str| {
            for p in text.split_whitespace() {
                if p == "!" {
                    out.clear();
                } else {
                    out.push(p.to_string());
                }
            }
        };
        if let Ok(env) = std::env::var("CVSIGNORE") {
            add(&env);
        }
        if let Some(home) = std::env::var_os("HOME") {
            add(&std::fs::read_to_string(Path::new(&home).join(".cvsignore")).unwrap_or_default());
        }
        add(&std::fs::read_to_string(dir.join(".cvsignore")).unwrap_or_default());
        out
    }

    /// Every file and folder under `dir` (relative path `rel`), each with its state, read from
    /// the working copy alone. Unversioned folders are listed, not entered.
    fn walk(&self, dir: &Path, rel: &str, out: &mut Vec<Change>) {
        let admin = dir.join("CVS");
        let entries = parse_entries(&std::fs::read_to_string(admin.join("Entries")).unwrap_or_default(), &std::fs::read_to_string(admin.join("Entries.Log")).unwrap_or_default());
        let join = |name: &str| if rel.is_empty() { name.to_string() } else { format!("{rel}/{name}") };
        let mut known: Vec<String> = Vec::new();
        for e in &entries {
            match e {
                Entry::File { name, revision, timestamp, conflict } => {
                    known.push(name.clone());
                    let path = dir.join(name);
                    let status = if *conflict {
                        Status::Conflicted
                    } else if revision == "0" {
                        Status::Added
                    } else if revision.starts_with('-') {
                        Status::Deleted
                    } else {
                        match std::fs::metadata(&path).and_then(|m| m.modified()) {
                            Err(_) => Status::Deleted,
                            Ok(t) => {
                                let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
                                if asctime(secs) == *timestamp { Status::Normal } else { Status::Modified }
                            }
                        }
                    };
                    out.push(Change { path: join(name), old_path: None, status, staged: false, added: None, removed: None });
                }
                Entry::Dir { name } => {
                    known.push(name.clone());
                    let sub = dir.join(name);
                    if sub.join("CVS").is_dir() {
                        self.walk(&sub, &join(name), out);
                    }
                }
            }
        }
        let patterns = self.ignore_patterns(dir);
        let Ok(read) = std::fs::read_dir(dir) else { return };
        for item in read.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if name == "CVS" || known.contains(&name) {
                continue;
            }
            // A folder CVS knows of but Entries forgot still carries its own CVS folder.
            let is_dir = item.file_type().is_ok_and(|t| t.is_dir());
            if is_dir && item.path().join("CVS").join("Entries").is_file() {
                self.walk(&item.path(), &join(&name), out);
                continue;
            }
            let status = if ignored_by(&patterns, &name) { Status::Ignored } else { Status::Unversioned };
            out.push(Change { path: join(&name), old_path: None, status, staged: false, added: None, removed: None });
        }
    }

    fn states(&self, rel: &str) -> Vec<Change> {
        let dir = if rel.is_empty() { self.root.clone() } else { self.root.join(rel) };
        let mut out = Vec::new();
        self.walk(&dir, rel, &mut out);
        out
    }

    /// The revision `CVS/Entries` records for a file: the one the working file was checked out at.
    fn entry_revision(&self, rel: &str) -> Option<String> {
        let (dir, name) = match rel.rsplit_once('/') {
            Some((d, n)) => (self.root.join(d), n),
            None => (self.root.clone(), rel),
        };
        let admin = dir.join("CVS");
        parse_entries(&std::fs::read_to_string(admin.join("Entries")).ok()?, &std::fs::read_to_string(admin.join("Entries.Log")).unwrap_or_default()).into_iter().find_map(|e| match e {
            Entry::File { name: n, revision, .. } if n == name && revision != "0" && !revision.starts_with('-') => Some(revision),
            _ => None,
        })
    }

    fn file_revisions(&self, paths: &[String]) -> Vec<FileRevision> {
        let mut args = vec!["-q", "log"];
        let targets: Vec<&str> = paths.iter().filter(|p| !p.is_empty()).map(String::as_str).collect();
        args.extend(targets);
        parse_log(&self.text(&args).unwrap_or_default())
    }
}

impl super::backend::Backend for Cvs {
    fn tool(&self) -> Tool {
        Tool::Cvs
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn branch_label(&self) -> String {
        match std::fs::read_to_string(self.root.join("CVS/Tag")) {
            Ok(t) => {
                let t = t.trim();
                t.get(1..).filter(|n| !n.is_empty()).unwrap_or("HEAD").to_string()
            }
            Err(_) => "HEAD".into(),
        }
    }

    fn changes(&self, paths: &[String], unversioned: bool, ignored: bool) -> Result<Vec<Change>, String> {
        let roots: Vec<String> = if paths.is_empty() { vec![String::new()] } else { paths.to_vec() };
        let mut list: Vec<Change> = Vec::new();
        for r in &roots {
            let full = self.root.join(r);
            if full.is_dir() {
                list.extend(self.states(r));
            } else {
                let parent = r.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                list.extend(self.states(parent).into_iter().filter(|c| c.path == *r));
            }
        }
        list.retain(|c| c.status != Status::Normal && (unversioned || c.status != Status::Unversioned) && (ignored || c.status != Status::Ignored));
        let mut args = vec!["-Q", "diff", "-u"];
        args.extend(paths.iter().filter(|p| !p.is_empty()).map(String::as_str));
        let counts = count_unified(&self.text(&args).unwrap_or_default(), "");
        for c in &mut list {
            if let Some(&(a, r)) = counts.get(&c.path) {
                (c.added, c.removed) = (a, r);
            } else if matches!(c.status, Status::Unversioned | Status::Added) {
                (c.added, c.removed) = super::state::new_file_lines(&self.root.join(&c.path));
            }
        }
        list.sort_by(|a, b| a.path.cmp(&b.path));
        list.dedup_by(|a, b| a.path == b.path);
        Ok(list)
    }

    fn folder_status(&self, dir: &Path) -> Result<HashMap<OsString, Status>, String> {
        let rel = self.relative(dir).ok_or("not in this working copy")?;
        let all = self.states(&rel);
        let tracked: Vec<String> = all.iter().filter(|c| !matches!(c.status, Status::Unversioned | Status::Ignored)).map(|c| c.path.clone()).collect();
        let changes: Vec<Change> = all.into_iter().filter(|c| c.status != Status::Normal).collect();
        Ok(folder_status_from(&rel, &changes, tracked.iter().map(String::as_str)))
    }

    fn summary(&self) -> Result<Status, String> {
        Ok(self.states("").iter().filter(|c| c.status.is_change()).map(|c| if c.status == Status::Conflicted { Status::Conflicted } else { Status::Modified }).max().unwrap_or(Status::Normal))
    }

    fn log(&self, paths: &[String], max: usize) -> Result<Vec<Commit>, String> {
        let sets = changesets(self.file_revisions(paths));
        let mut out: Vec<Commit> = sets
            .iter()
            .take(max)
            .map(|c| {
                let first = &c.revisions[0];
                Commit { hash: c.id.clone(), parents: Vec::new(), author: first.author.clone(), email: String::new(), time: first.time, committer: first.author.clone(), commit_time: first.time, message: first.message.clone() }
            })
            .collect();
        for i in 0..out.len() {
            if let Some(older) = out.get(i + 1).map(|o| o.hash.clone()) {
                out[i].parents = vec![older];
            }
        }
        Ok(out)
    }

    fn log_actions(&self, paths: &[String], max: usize) -> HashMap<String, Vec<char>> {
        changesets(self.file_revisions(paths))
            .into_iter()
            .take(max)
            .map(|c| {
                let mut kinds: Vec<char> = c.revisions.iter().map(|r| if r.dead { 'D' } else if previous_revision(&r.revision).is_none() { 'A' } else { 'M' }).collect();
                kinds.sort_by_key(|k| "AMDR".find(*k));
                kinds.dedup();
                (c.id, kinds)
            })
            .collect()
    }

    fn revision_changes(&self, rev: &str) -> Result<Vec<Change>, String> {
        let set = changesets(self.file_revisions(&[])).into_iter().find(|c| c.id == rev).ok_or_else(|| format!("no revision {rev}"))?;
        Ok(set
            .revisions
            .iter()
            .map(|r| Change {
                path: r.path.clone(),
                old_path: None,
                status: if r.dead { Status::Deleted } else if previous_revision(&r.revision).is_none() { Status::Added } else { Status::Modified },
                staged: false,
                added: r.added,
                removed: r.removed,
            })
            .collect())
    }

    fn parent(&self, rev: &str) -> Option<String> {
        if rev.contains('.') {
            return previous_revision(rev);
        }
        let sets = changesets(self.file_revisions(&[]));
        let at = sets.iter().position(|c| c.id == rev)?;
        sets.get(at + 1).map(|c| c.id.clone())
    }

    fn blame(&self, path: &str) -> Result<Vec<BlameLine>, String> {
        let mut lines = parse_annotate(&self.text(&["-Q", "annotate", path])?);
        let messages: HashMap<String, String> = self.file_revisions(&[path.to_string()]).into_iter().map(|r| (r.revision, r.message.lines().next().unwrap_or("").to_string())).collect();
        for l in &mut lines {
            l.summary = messages.get(&l.hash).cloned().unwrap_or_default();
        }
        Ok(lines)
    }

    fn base_text(&self, path: &str) -> Vec<u8> {
        match self.entry_revision(path) {
            Some(rev) => self.text_at(&rev, path),
            None => Vec::new(),
        }
    }

    fn text_at(&self, rev: &str, path: &str) -> Vec<u8> {
        if rev.is_empty() {
            return Vec::new();
        }
        let flag = date_arg(rev);
        let args: Vec<&str> = if rev.contains('.') { vec!["-Q", "update", "-p", "-r", rev, path] } else { vec!["-Q", "update", "-p", "-D", &flag, path] };
        self.client(&args).unwrap_or_default()
    }

    fn last_two_versions(&self, path: &str) -> Option<(String, String)> {
        let mut revs = self.file_revisions(&[path.to_string()]);
        revs.sort_by(|a, b| b.time.cmp(&a.time));
        let mut it = revs.into_iter().filter(|r| !r.dead).map(|r| r.revision);
        let latest = it.next()?;
        Some((latest, it.next().unwrap_or_default()))
    }

    fn has_conflicts(&self) -> bool {
        self.states("").iter().any(|c| c.status == Status::Conflicted)
    }

    fn admin_dir(&self) -> Option<PathBuf> {
        Some(self.root.join("CVS"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_name_files_folders_and_their_marks() {
        let e = parse_entries("/one.txt/1.2/Sat Nov 14 02:33:19 2026//\n/new.txt/0/dummy timestamp//\n/gone.txt/-1.1/Sat Nov 14 02:33:19 2026//\n/c.txt/1.3/Result of merge+Sat Nov 14 02:33:19 2026//\nD/sub////\nD\n", "A D/later////\nR /new.txt/0/dummy timestamp//\n");
        assert!(e.contains(&Entry::File { name: "one.txt".into(), revision: "1.2".into(), timestamp: "Sat Nov 14 02:33:19 2026".into(), conflict: false }));
        assert!(e.iter().any(|x| matches!(x, Entry::File { name, conflict: true, .. } if name == "c.txt")));
        assert!(e.contains(&Entry::Dir { name: "sub".into() }));
        assert!(e.contains(&Entry::Dir { name: "later".into() }), "Entries.Log adds");
        assert!(!e.iter().any(|x| entry_name(x) == "new.txt"), "and removes");
    }

    #[test]
    fn asctime_writes_what_cvs_compares() {
        assert_eq!(asctime(0), "Thu Jan  1 00:00:00 1970");
        assert_eq!(asctime(828_840_566), "Sun Apr  7 01:29:26 1996");
        assert_eq!(asctime(1_794_623_599), "Sat Nov 14 02:33:19 2026");
    }

    #[test]
    fn both_log_date_forms_read_the_same() {
        assert_eq!(log_time("2026/11/14 02:33:19"), 1_794_623_599);
        assert_eq!(log_time("2026-11-14 02:33:19 +0000"), 1_794_623_599);
    }

    #[test]
    fn ignore_patterns_are_sh_wildcards() {
        let p: Vec<String> = ["*.o", "core", "build[0-9]", "?x"].iter().map(|s| s.to_string()).collect();
        for yes in ["a.o", "core", "build7", "ax"] {
            assert!(ignored_by(&p, yes), "{yes}");
        }
        for no in ["a.oo", "cores", "buildx", "abx"] {
            assert!(!ignored_by(&p, no), "{no}");
        }
    }

    const LOG: &str = "
RCS file: /r/mod/one.txt,v
Working file: one.txt
head: 1.2
branch:
locks: strict
access list:
symbolic names:
keyword substitution: kv
total revisions: 2;\tselected revisions: 2
description:
----------------------------
revision 1.2
date: 2026/11/14 02:33:19;  author: fixture;  state: Exp;  lines: +2 -1
second
----------------------------
revision 1.1
date: 2026/11/14 02:33:17;  author: fixture;  state: Exp;
first
=============================================================================

RCS file: /r/mod/two.txt,v
Working file: two.txt
head: 1.1
description:
----------------------------
revision 1.1
date: 2026/11/14 02:33:18;  author: fixture;  state: Exp;
first
=============================================================================
";

    #[test]
    fn the_log_lists_every_file_revision() {
        let r = parse_log(LOG);
        assert_eq!(r.len(), 3);
        assert_eq!((r[0].path.as_str(), r[0].revision.as_str(), r[0].added, r[0].removed, r[0].message.as_str()), ("one.txt", "1.2", Some(2), Some(1), "second"));
        assert_eq!(r[0].time, 1_794_623_599);
    }

    #[test]
    fn revisions_sharing_author_and_message_within_five_minutes_are_one_changeset() {
        let sets = changesets(parse_log(LOG));
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[0].revisions.len(), 1);
        assert_eq!(sets[1].revisions.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), ["two.txt", "one.txt"]);
        assert_eq!(sets[1].id, "1794623598");
    }

    #[test]
    fn annotate_lines_carry_revision_author_and_text() {
        let b = parse_annotate("1.1          (fixture  14-Nov-26): alpha\n1.2          (fixture  14-Nov-26): beta changed\n1.2          (fixture  14-Nov-26): \n");
        assert_eq!(b.iter().map(|l| (l.hash.as_str(), l.text.as_str())).collect::<Vec<_>>(), [("1.1", "alpha"), ("1.2", "beta changed"), ("1.2", "")]);
        assert_eq!(b[0].time, 1_794_614_400);
    }

    #[test]
    fn a_revision_s_predecessor() {
        assert_eq!(previous_revision("1.2").as_deref(), Some("1.1"));
        assert_eq!(previous_revision("1.1"), None);
        assert_eq!(previous_revision("1.2.2.1").as_deref(), Some("1.2"));
    }
}
