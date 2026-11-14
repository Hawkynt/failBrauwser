//! Git working trees, read and changed through the `git` command: the status of the files
//! shown (for the overlay icons), the changes to commit, the history, blame and diffs. The
//! dialogs in `ui/git` show what this reads; nothing here needs a display.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub use super::state::*;

/// The tree of a commit without files: what a repository without commits is compared with.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// A working tree and where its repository data is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Repo {
    pub root: PathBuf,
    pub git_dir: PathBuf,
}

/// `git` with a fixed language for the parts parsed, no questions on the terminal and no
/// locks taken where only reading.
pub fn command(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(dir).env("LC_ALL", "C").env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null());
    c
}

/// Runs git in `dir` and returns what it printed, or its error message.
pub fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = command(dir).env("GIT_OPTIONAL_LOCKS", "0").args(args).output().map_err(|e| format!("git cannot be started: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { format!("git {} failed", args.first().unwrap_or(&"")) } else { err })
    }
}

/// Whether the git command is there at all (asked once).
pub fn available() -> bool {
    static FOUND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| Command::new("git").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success()))
}

/// The version git reports ("2.56.1").
pub fn version() -> Option<String> {
    let out = Command::new("git").arg("--version").output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    s.trim().strip_prefix("git version ").map(str::to_string)
}

/// The working tree holding `path` (a file or folder), found by looking for `.git` without
/// running git. Folders inside `.git` belong to none.
pub fn discover(path: &Path) -> Option<Repo> {
    if path.components().any(|c| c.as_os_str() == ".git") {
        return None;
    }
    let mut dir = if path.is_dir() { Some(path) } else { path.parent() };
    while let Some(d) = dir {
        if let Some(r) = repo_at(d) {
            return Some(r);
        }
        dir = d.parent();
    }
    None
}

/// The working tree whose top folder is `dir`, if it is one.
pub fn repo_at(dir: &Path) -> Option<Repo> {
    let dot = dir.join(".git");
    let meta = std::fs::metadata(&dot).ok()?;
    if meta.is_dir() {
        return dot.join("HEAD").is_file().then(|| Repo { root: dir.to_path_buf(), git_dir: dot });
    }
    // Worktrees and submodules: a file naming the repository data elsewhere.
    let text = std::fs::read_to_string(&dot).ok()?;
    let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let git_dir = if Path::new(target).is_absolute() { PathBuf::from(target) } else { dir.join(target) };
    git_dir.join("HEAD").is_file().then(|| Repo { root: dir.to_path_buf(), git_dir })
}

impl Repo {
    /// `path` relative to the top of the working tree, with '/' ("" for the top itself).
    pub fn relative(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.root).ok()?;
        Some(rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
    }

    pub fn run(&self, args: &[&str]) -> Result<String, String> {
        run(&self.root, args)
    }

    /// The branch checked out, from HEAD; None when detached.
    pub fn branch(&self) -> Option<String> {
        let head = std::fs::read_to_string(self.git_dir.join("HEAD")).ok()?;
        head.trim().strip_prefix("ref: refs/heads/").map(str::to_string)
    }

    /// What TortoiseGit's "Git Commit -> …" names: the branch, or "(no branch)".
    pub fn branch_label(&self) -> String {
        self.branch().unwrap_or_else(|| "(no branch)".into())
    }

    /// The repository data shared by all worktrees.
    pub fn common_dir(&self) -> PathBuf {
        match std::fs::read_to_string(self.git_dir.join("commondir")) {
            Ok(t) => {
                let p = Path::new(t.trim());
                if p.is_absolute() { p.to_path_buf() } else { self.git_dir.join(p) }
            }
            Err(_) => self.git_dir.clone(),
        }
    }

    /// Whether HEAD names a commit (false in a repository without commits).
    pub fn has_commits(&self) -> bool {
        self.run(&["rev-parse", "--verify", "-q", "HEAD"]).is_ok()
    }

    /// What the working tree is compared with: HEAD, or the empty tree before the first commit.
    pub fn base(&self) -> &'static str {
        if self.has_commits() { "HEAD" } else { EMPTY_TREE }
    }

    /// The changes of the working tree under `paths` (all of it when empty), with their
    /// line counts; unversioned and ignored files when asked.
    pub fn changes(&self, paths: &[String], unversioned: bool, ignored: bool) -> Result<Vec<Change>, String> {
        let mut args = vec!["status", "--porcelain=v2", "-z"];
        args.push(if unversioned { "--untracked-files=all" } else { "--untracked-files=no" });
        if ignored {
            args.push("--ignored=traditional");
        }
        args.push("--");
        let specs = pathspecs(paths);
        args.extend(specs.iter().map(String::as_str));
        let mut list = parse_status(&self.run(&args)?);
        if !ignored {
            list.retain(|c| c.status != Status::Ignored);
        }
        let mut nargs = vec!["diff", "--numstat", "-z", "-M", self.base(), "--"];
        nargs.extend(specs.iter().map(String::as_str));
        let counts = parse_numstat(&self.run(&nargs).unwrap_or_default());
        for c in &mut list {
            if let Some(&(a, r)) = counts.get(&c.path) {
                c.added = a;
                c.removed = r;
            } else if c.status == Status::Unversioned || (c.status == Status::Added && c.added.is_none()) {
                // New files: every line is added.
                if let Ok(text) = std::fs::read(self.root.join(&c.path)) {
                    if !text.contains(&0) {
                        c.added = Some(text.iter().filter(|&&b| b == b'\n').count() as u64 + u64::from(!text.is_empty() && !text.ends_with(b"\n")));
                        c.removed = Some(0);
                    }
                }
            }
        }
        list.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(list)
    }

    /// The overlay status of each entry of the folder `dir` (inside this working tree):
    /// files by their own state, folders by the most important state of what they hold.
    /// Folders that are working trees of their own are left out (see [`summary`]).
    pub fn folder_status(&self, dir: &Path) -> Result<HashMap<OsString, Status>, String> {
        let rel = self.relative(dir).ok_or("not in this working tree")?;
        let spec = if rel.is_empty() { ".".to_string() } else { rel.clone() };
        let status = self.run(&["status", "--porcelain=v2", "-z", "--untracked-files=normal", "--ignored=matching", "--", &spec])?;
        let tracked = self.run(&["ls-files", "-z", "--", &spec])?;
        Ok(folder_status_from(&rel, &parse_status(&status), tracked.split('\0').filter(|s| !s.is_empty())))
    }

    /// Names of local branches, remote branches (`origin/main`) and tags.
    pub fn refs(&self) -> Refs {
        let mut r = Refs::default();
        let text = self.run(&["for-each-ref", "--format=%(objectname) %(*objectname) %(refname)"]).unwrap_or_default();
        for line in text.lines() {
            let mut it = line.splitn(3, ' ');
            let (Some(obj), Some(peeled), Some(name)) = (it.next(), it.next(), it.next()) else { continue };
            let target = if peeled.is_empty() { obj } else { peeled }.to_string();
            if let Some(n) = name.strip_prefix("refs/heads/") {
                r.branches.push(n.to_string());
                r.by_commit.entry(target).or_default().push(RefLabel::Branch(n.to_string()));
            } else if let Some(n) = name.strip_prefix("refs/remotes/") {
                if n.ends_with("/HEAD") {
                    continue;
                }
                r.remote_branches.push(n.to_string());
                r.by_commit.entry(target).or_default().push(RefLabel::Remote(n.to_string()));
            } else if let Some(n) = name.strip_prefix("refs/tags/") {
                r.tags.push(n.to_string());
                r.by_commit.entry(target).or_default().push(RefLabel::Tag(n.to_string()));
            } else if name == "refs/stash" {
                r.by_commit.entry(target).or_default().push(RefLabel::Stash);
            }
        }
        r
    }

    pub fn remotes(&self) -> Vec<String> {
        self.run(&["remote"]).unwrap_or_default().lines().map(str::to_string).collect()
    }

    pub fn remote_url(&self, remote: &str) -> String {
        self.run(&["remote", "get-url", remote]).unwrap_or_default().trim().to_string()
    }

    /// The branch `branch` pulls from ("origin/main"), if it tracks one.
    pub fn upstream(&self, branch: &str) -> Option<String> {
        let s = self.run(&["rev-parse", "--abbrev-ref", &format!("{branch}@{{upstream}}")]).ok()?;
        Some(s.trim().to_string()).filter(|s| !s.is_empty())
    }

    pub fn has_stash(&self) -> bool {
        self.run(&["rev-parse", "--verify", "-q", "refs/stash"]).is_ok()
    }

    /// A merge, rebase, cherry-pick or revert waits for its conflicts to be resolved.
    pub fn has_conflicts(&self) -> bool {
        self.run(&["ls-files", "-u"]).is_ok_and(|s| !s.trim().is_empty())
    }

    /// The history, newest first: of `rev` (HEAD) or of all branches, only commits that
    /// changed `paths` when any are given.
    pub fn log(&self, all: bool, paths: &[String], max: usize) -> Result<Vec<Commit>, String> {
        let max = format!("--max-count={max}");
        let mut args = vec!["log", "--topo-order", "-z", &max, "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ct%x1f%B"];
        if all {
            args.push("--all");
        }
        if !self.has_commits() && !all {
            return Ok(Vec::new());
        }
        args.push("--");
        let specs = pathspecs(paths);
        args.extend(specs.iter().map(String::as_str));
        Ok(parse_log(&self.run(&args)?))
    }

    /// The commits of a range such as "origin/main..HEAD", newest first.
    pub fn log_range(&self, range: &str) -> Vec<Commit> {
        parse_log(&self.run(&["log", "-z", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ct%x1f%B", range, "--"]).unwrap_or_default())
    }

    /// What kinds of change each commit of the history made ('A', 'M', 'D', 'R'), for
    /// the log's Actions column.
    pub fn log_actions(&self, all: bool, paths: &[String], max: usize) -> HashMap<String, Vec<char>> {
        let max = format!("--max-count={max}");
        let mut args = vec!["log", "--topo-order", &max, "--format=%x1e%H", "--name-status", "-M"];
        if all {
            args.push("--all");
        }
        args.push("--");
        let specs = pathspecs(paths);
        args.extend(specs.iter().map(String::as_str));
        parse_log_actions(&self.run(&args).unwrap_or_default())
    }

    /// What a commit changed (against its first parent), with line counts.
    pub fn commit_changes(&self, hash: &str) -> Result<Vec<Change>, String> {
        let parent = self.run(&["rev-parse", "--verify", "-q", &format!("{hash}^1")]).ok().map(|s| s.trim().to_string());
        let base = parent.as_deref().unwrap_or(EMPTY_TREE);
        let names = self.run(&["diff-tree", "-r", "-M", "-z", "--name-status", base, hash])?;
        let counts = parse_numstat(&self.run(&["diff-tree", "-r", "-M", "-z", "--numstat", base, hash])?);
        let mut list = parse_name_status(&names);
        for c in &mut list {
            if let Some(&(a, r)) = counts.get(&c.path) {
                c.added = a;
                c.removed = r;
            }
        }
        Ok(list)
    }

    /// Who last changed each line of `path` (at `rev`, or as in the working tree).
    pub fn blame(&self, path: &str, rev: Option<&str>) -> Result<Vec<BlameLine>, String> {
        let mut args = vec!["blame", "--porcelain"];
        if let Some(r) = rev {
            args.push(r);
        }
        args.push("--");
        args.push(path);
        Ok(parse_blame(&self.run(&args)?))
    }

    /// The text of `path` at `rev` ("" when it did not exist there).
    pub fn show_file(&self, rev: &str, path: &str) -> Vec<u8> {
        let out = command(&self.root).args(["show", &format!("{rev}:{path}")]).output();
        out.ok().filter(|o| o.status.success()).map(|o| o.stdout).unwrap_or_default()
    }

    /// The two commits that last changed `path`: (the latest, the one before it).
    pub fn last_two_versions(&self, path: &str) -> Option<(String, String)> {
        let text = self.run(&["log", "-2", "--format=%H", "--follow", "--", path]).ok()?;
        let mut it = text.lines();
        let latest = it.next()?.to_string();
        let before = it.next().map(str::to_string).unwrap_or_else(|| EMPTY_TREE.to_string());
        Some((latest, before))
    }

    /// The reflog of HEAD, newest first.
    pub fn reflog(&self) -> Result<Vec<ReflogEntry>, String> {
        let text = self.run(&["reflog", "-z", "--format=%H%x1f%gd%x1f%gs%x1f%ct"])?;
        Ok(text
            .split('\0')
            .filter(|s| !s.trim().is_empty())
            .filter_map(|rec| {
                let f: Vec<&str> = rec.trim_start_matches('\n').split('\x1f').collect();
                let (action, message) = match f.get(2)?.split_once(": ") {
                    Some((a, m)) => (a.to_string(), m.to_string()),
                    None => (f[2].to_string(), String::new()),
                };
                Some(ReflogEntry { hash: f[0].to_string(), selector: f.get(1)?.to_string(), action, message, time: f.get(3)?.trim().parse().unwrap_or(0) })
            })
            .collect())
    }

    /// The stashes, newest first: (stash@{n}, its message).
    pub fn stashes(&self) -> Vec<(String, String)> {
        let text = self.run(&["stash", "list", "--format=%gd%x1f%gs"]).unwrap_or_default();
        text.lines().filter_map(|l| l.split_once('\x1f')).map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    /// A configuration value as git uses it (local over global), or from one file only
    /// (`scope`: "local" or "global").
    pub fn config(&self, key: &str, scope: Option<&str>) -> String {
        let flag = scope.map(|s| format!("--{s}"));
        let mut args = vec!["config"];
        if let Some(f) = &flag {
            args.push(f);
        }
        args.push("--get");
        args.push(key);
        self.run(&args).unwrap_or_default().trim().to_string()
    }
}

/// `paths` as pathspecs taken literally (no wildcards), relative to the top of the tree.
fn pathspecs(paths: &[String]) -> Vec<String> {
    paths.iter().map(|p| if p.is_empty() { ":(top)".to_string() } else { format!(":(top,literal){p}") }).collect()
}

/// Whether a folder holding a git working tree of its own is clean, changed or in conflict
/// (unversioned files do not count, as in TortoiseGit).
pub fn summary(root: &Path) -> Result<Status, String> {
    let text = run(root, &["status", "--porcelain=v2", "-z", "--untracked-files=no", "--ignore-submodules=dirty"])?;
    let list = parse_status(&text);
    Ok(list.iter().map(|c| if c.status == Status::Conflicted { Status::Conflicted } else { Status::Modified }).max().unwrap_or(Status::Normal))
}

/// Parses `git status --porcelain=v2 -z`.
pub fn parse_status(text: &str) -> Vec<Change> {
    let mut out = Vec::new();
    let mut fields = text.split('\0');
    while let Some(rec) = fields.next() {
        let Some(kind) = rec.chars().next() else { continue };
        let change = |status, path: &str, old: Option<String>, staged| Change { path: path.trim_end_matches('/').to_string(), old_path: old, status, staged, added: None, removed: None };
        match kind {
            '1' => {
                // 1 XY sub mH mI mW hH hI path
                let parts: Vec<&str> = rec.splitn(9, ' ').collect();
                if parts.len() < 9 {
                    continue;
                }
                let xy = parts[1].as_bytes();
                out.push(change(xy_status(xy), parts[8], None, xy[0] != b'.'));
            }
            '2' => {
                // 2 XY sub mH mI mW hH hI Xscore path, then the original path
                let parts: Vec<&str> = rec.splitn(10, ' ').collect();
                let old = fields.next().map(str::to_string);
                if parts.len() < 10 {
                    continue;
                }
                let xy = parts[1].as_bytes();
                let st = if xy.contains(&b'D') { Status::Deleted } else { Status::Renamed };
                out.push(change(st, parts[9], old, xy[0] != b'.'));
            }
            'u' => {
                let parts: Vec<&str> = rec.splitn(11, ' ').collect();
                if let Some(p) = parts.get(10) {
                    out.push(change(Status::Conflicted, p, None, false));
                }
            }
            '?' => out.push(change(Status::Unversioned, &rec[2..], None, false)),
            '!' => out.push(change(Status::Ignored, &rec[2..], None, false)),
            _ => {}
        }
    }
    out
}

fn xy_status(xy: &[u8]) -> Status {
    let (x, y) = (xy.first().copied().unwrap_or(b'.'), xy.get(1).copied().unwrap_or(b'.'));
    if x == b'U' || y == b'U' || (x == b'A' && y == b'A') || (x == b'D' && y == b'D') {
        Status::Conflicted
    } else if x == b'D' || y == b'D' {
        Status::Deleted
    } else if x == b'A' {
        Status::Added
    } else if x == b'R' || x == b'C' {
        Status::Renamed
    } else {
        Status::Modified
    }
}

/// Parses `--numstat -z`: path → (added, removed), None for binary files. Renamed files
/// are listed under their new path.
pub fn parse_numstat(text: &str) -> HashMap<String, (Option<u64>, Option<u64>)> {
    let mut out = HashMap::new();
    let mut fields = text.split('\0');
    while let Some(rec) = fields.next() {
        let rec = rec.trim_start_matches('\n');
        let mut it = rec.splitn(3, '\t');
        let (Some(a), Some(r), Some(path)) = (it.next(), it.next(), it.next()) else { continue };
        let path = if path.is_empty() {
            // A rename: the old path, then the new one, in fields of their own.
            let _old = fields.next();
            fields.next().unwrap_or_default().to_string()
        } else {
            path.to_string()
        };
        out.insert(path, (a.parse().ok(), r.parse().ok()));
    }
    out
}

/// Parses `--name-status -z`.
pub fn parse_name_status(text: &str) -> Vec<Change> {
    let mut out = Vec::new();
    let mut fields = text.split('\0').filter(|s| !s.is_empty());
    while let Some(code) = fields.next() {
        let c = code.as_bytes()[0];
        let (status, old) = match c {
            b'A' => (Status::Added, None),
            b'D' => (Status::Deleted, None),
            b'R' | b'C' => (Status::Renamed, fields.next().map(str::to_string)),
            b'U' => (Status::Conflicted, None),
            _ => (Status::Modified, None),
        };
        let Some(path) = fields.next() else { break };
        out.push(Change { path: path.to_string(), old_path: old, status, staged: true, added: None, removed: None });
    }
    out
}

pub fn parse_log(text: &str) -> Vec<Commit> {
    text.split('\0')
        .filter(|r| !r.trim().is_empty())
        .filter_map(|rec| {
            let f: Vec<&str> = rec.trim_start_matches('\n').splitn(8, '\x1f').collect();
            if f.len() < 8 {
                return None;
            }
            Some(Commit {
                hash: f[0].to_string(),
                parents: f[1].split_whitespace().map(str::to_string).collect(),
                author: f[2].to_string(),
                email: f[3].to_string(),
                time: f[4].parse().unwrap_or(0),
                committer: f[5].to_string(),
                commit_time: f[6].parse().unwrap_or(0),
                message: f[7].trim_end().to_string(),
            })
        })
        .collect()
}

/// Parses the history with `--name-status` after "\x1e<hash>" lines: the kinds of
/// change of each commit, in the order A, M, D, R.
pub fn parse_log_actions(text: &str) -> HashMap<String, Vec<char>> {
    let mut out = HashMap::new();
    for rec in text.split('\x1e').filter(|r| !r.trim().is_empty()) {
        let mut lines = rec.lines();
        let Some(hash) = lines.next().map(str::trim) else { continue };
        let mut kinds: Vec<char> = Vec::new();
        for l in lines {
            let k = match l.chars().next() {
                Some('A') => 'A',
                Some('D') => 'D',
                Some('R') | Some('C') => 'R',
                Some('M') | Some('T') | Some('U') => 'M',
                _ => continue,
            };
            if !kinds.contains(&k) {
                kinds.push(k);
            }
        }
        kinds.sort_by_key(|k| "AMDR".find(*k));
        out.insert(hash.to_string(), kinds);
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflogEntry {
    pub hash: String,
    /// "HEAD@{0}"
    pub selector: String,
    /// "commit", "checkout", "reset", …
    pub action: String,
    pub message: String,
    pub time: i64,
}

pub fn parse_blame(text: &str) -> Vec<BlameLine> {
    let mut info: HashMap<String, (String, i64, String)> = HashMap::new();
    let mut out = Vec::new();
    let mut cur: Option<(String, usize)> = None;
    for line in text.lines() {
        if let Some(t) = line.strip_prefix('\t') {
            if let Some((hash, n)) = cur.take() {
                let (author, time, summary) = info.get(&hash).cloned().unwrap_or_default();
                out.push(BlameLine { hash, author, time, summary, line: n, text: t.to_string() });
            }
            continue;
        }
        let mut words = line.split(' ');
        let first = words.next().unwrap_or("");
        if first.len() == 40 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
            let _orig = words.next();
            let n = words.next().and_then(|x| x.parse().ok()).unwrap_or(0);
            info.entry(first.to_string()).or_default();
            cur = Some((first.to_string(), n));
        } else if let Some((hash, _)) = &cur {
            let e = info.entry(hash.clone()).or_default();
            let rest = line.split_once(' ').map(|x| x.1).unwrap_or("");
            match first {
                "author" => e.0 = rest.to_string(),
                "author-time" => e.1 = rest.parse().unwrap_or(0),
                "summary" => e.2 = rest.to_string(),
                _ => {}
            }
        }
    }
    out
}

/// The URL's last part without ".git": the folder a clone goes into.
pub fn clone_dir_name(url: &str) -> String {
    let t = url.trim().trim_end_matches('/').trim_end_matches(".git").trim_end_matches('/');
    let last = t.rsplit(['/', ':', '\\']).next().unwrap_or(t);
    if last.is_empty() { "repository".into() } else { last.to_string() }
}

/// Git as one tool among the five: the same questions, answered by the methods above.
impl super::backend::Backend for Repo {
    fn tool(&self) -> super::Tool {
        super::Tool::Git
    }
    fn root(&self) -> &Path {
        &self.root
    }
    fn branch_label(&self) -> String {
        Repo::branch_label(self)
    }
    fn changes(&self, paths: &[String], unversioned: bool, ignored: bool) -> Result<Vec<Change>, String> {
        Repo::changes(self, paths, unversioned, ignored)
    }
    fn folder_status(&self, dir: &Path) -> Result<HashMap<OsString, Status>, String> {
        Repo::folder_status(self, dir)
    }
    fn summary(&self) -> Result<Status, String> {
        summary(&self.root)
    }
    fn log(&self, paths: &[String], max: usize) -> Result<Vec<Commit>, String> {
        Repo::log(self, false, paths, max)
    }
    fn log_actions(&self, paths: &[String], max: usize) -> HashMap<String, Vec<char>> {
        Repo::log_actions(self, false, paths, max)
    }
    fn revision_changes(&self, rev: &str) -> Result<Vec<Change>, String> {
        self.commit_changes(rev)
    }
    fn parent(&self, rev: &str) -> Option<String> {
        self.run(&["rev-parse", "--verify", "-q", &format!("{rev}^1")]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }
    fn blame(&self, path: &str) -> Result<Vec<BlameLine>, String> {
        // As TortoiseGitBlame shows it: the committed file, not the edits on top of it.
        Repo::blame(self, path, if self.has_commits() { Some("HEAD") } else { None })
    }
    fn base_text(&self, path: &str) -> Vec<u8> {
        self.show_file("HEAD", path)
    }
    fn text_at(&self, rev: &str, path: &str) -> Vec<u8> {
        if rev.is_empty() || rev == EMPTY_TREE { Vec::new() } else { self.show_file(rev, path) }
    }
    fn last_two_versions(&self, path: &str) -> Option<(String, String)> {
        Repo::last_two_versions(self, path).map(|(a, b)| (a, if b == EMPTY_TREE { String::new() } else { b }))
    }
    fn has_conflicts(&self) -> bool {
        Repo::has_conflicts(self)
    }
    fn refs(&self) -> Refs {
        Repo::refs(self)
    }
    fn admin_dir(&self) -> Option<PathBuf> {
        Some(self.git_dir.clone())
    }
    fn as_git(&self) -> Option<&Repo> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines_become_changes() {
        let text = "1 .M N... 100644 100644 100644 a a src/main.rs\0\
                    1 A. N... 000000 100644 100644 0 b new.txt\0\
                    1 .D N... 100644 100644 000000 a a gone.txt\0\
                    2 R. N... 100644 100644 100644 a a R100 b.txt\0a.txt\0\
                    u UU N... 100644 100644 100644 100644 a b c both.txt\0\
                    ? notes.md\0\
                    ! target/\0";
        let c = parse_status(text);
        let got: Vec<(&str, Status)> = c.iter().map(|c| (c.path.as_str(), c.status)).collect();
        assert_eq!(
            got,
            [
                ("src/main.rs", Status::Modified),
                ("new.txt", Status::Added),
                ("gone.txt", Status::Deleted),
                ("b.txt", Status::Renamed),
                ("both.txt", Status::Conflicted),
                ("notes.md", Status::Unversioned),
                ("target", Status::Ignored),
            ]
        );
        assert_eq!(c[3].old_path.as_deref(), Some("a.txt"));
        assert!(!c[0].staged && c[1].staged);
    }

    #[test]
    fn folders_take_the_state_of_what_they_hold() {
        let changes = parse_status("1 .M N... 100644 100644 100644 a a src/ui/x.rs\0? src/new.rs\0? docs/\0! target/\0u UU N... 1 1 1 1 a b c lib/c.rs\0");
        let tracked = ["src/ui/x.rs", "src/main.rs", "README.md", "lib/c.rs", "lib/d.rs"];
        let top = folder_status_from("", &changes, tracked.iter().copied());
        assert_eq!(top[&OsString::from("src")], Status::Modified);
        assert_eq!(top[&OsString::from("README.md")], Status::Normal);
        assert_eq!(top[&OsString::from("docs")], Status::Unversioned);
        assert_eq!(top[&OsString::from("target")], Status::Ignored);
        assert_eq!(top[&OsString::from("lib")], Status::Conflicted);
        let src = folder_status_from("src", &changes, tracked.iter().copied());
        assert_eq!(src[&OsString::from("ui")], Status::Modified);
        assert_eq!(src[&OsString::from("main.rs")], Status::Normal);
        assert_eq!(src[&OsString::from("new.rs")], Status::Unversioned);
        assert_eq!(src.len(), 3);
    }

    #[test]
    fn numstat_with_renames_and_binaries() {
        let m = parse_numstat("3\t1\tsrc/a.rs\0-\t-\timg.png\0\x35\t0\t\0old.txt\0new.txt\0");
        assert_eq!(m["src/a.rs"], (Some(3), Some(1)));
        assert_eq!(m["img.png"], (None, None));
        assert_eq!(m["new.txt"], (Some(5), Some(0)));
    }

    #[test]
    fn name_status_of_a_commit() {
        let c = parse_name_status("M\0a.txt\0A\0b.txt\0R100\0old\0new\0D\0c.txt\0");
        let got: Vec<(&str, Status)> = c.iter().map(|c| (c.path.as_str(), c.status)).collect();
        assert_eq!(got, [("a.txt", Status::Modified), ("b.txt", Status::Added), ("new", Status::Renamed), ("c.txt", Status::Deleted)]);
    }

    #[test]
    fn log_records() {
        let text = "aaa\x1fbbb ccc\x1fAnn\x1fann@x\x1f100\x1fBob\x1f200\x1fSubject\n\nBody\n\0\nddd\x1f\x1fAnn\x1fann@x\x1f50\x1fAnn\x1f50\x1fFirst\n\0";
        let l = parse_log(text);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].parents, ["bbb", "ccc"]);
        assert_eq!(l[0].subject(), "Subject");
        assert_eq!(l[0].message, "Subject\n\nBody");
        assert!(l[1].parents.is_empty());
    }

    #[test]
    fn actions_of_each_commit() {
        let a = parse_log_actions("\x1eaaa\n\nM\tx\nA\ty\nM\tz\n\x1ebbb\n\nR100\to\tn\nD\tq\n\x1eccc\n");
        assert_eq!(a["aaa"], ['A', 'M']);
        assert_eq!(a["bbb"], ['D', 'R']);
        assert!(a["ccc"].is_empty());
    }

    #[test]
    fn blame_porcelain() {
        let h1 = "1".repeat(40);
        let h2 = "2".repeat(40);
        let text = format!("{h1} 1 1 2\nauthor Ann\nauthor-time 100\nsummary first\nfilename f\n\tline one\n{h1} 2 2\n\tline two\n{h2} 3 3 1\nauthor Bob\nauthor-time 200\nsummary second\nfilename f\n\tline three\n");
        let b = parse_blame(&text);
        assert_eq!(b.len(), 3);
        assert_eq!((b[1].author.as_str(), b[1].line, b[1].text.as_str()), ("Ann", 2, "line two"));
        assert_eq!((b[2].author.as_str(), b[2].time, b[2].summary.as_str()), ("Bob", 200, "second"));
    }

    #[test]
    fn side_by_side_pairs_changed_lines() {
        let rows = side_by_side("a\nb\nc\nd\n", "a\nB\nc\nd\ne\n");
        let kinds: Vec<DiffKind> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, [DiffKind::Same, DiffKind::Changed, DiffKind::Same, DiffKind::Same, DiffKind::Added]);
        assert_eq!(rows[1].left, Some((2, "b".into())));
        assert_eq!(rows[1].right, Some((2, "B".into())));
        assert_eq!(rows[4].left, None);
        let removed = side_by_side("x\ny\nz\n", "x\nz\n");
        assert_eq!(removed[1].kind, DiffKind::Removed);
        assert_eq!(removed[2].left, Some((3, "z".into())));
        assert_eq!(removed[2].right, Some((2, "z".into())));
        assert!(side_by_side("", "").is_empty());
        assert_eq!(side_by_side("", "n\n")[0].kind, DiffKind::Added);
    }

    #[test]
    fn myers_finds_a_shortest_script() {
        let a = ["a", "b", "c", "a", "b", "b", "a"];
        let b = ["c", "b", "a", "b", "a", "c"];
        let ops = diff_lines(&a, &b);
        let edits = ops.iter().filter(|o| **o != Op::Same).count();
        assert_eq!(edits, 5);
        // Replaying the script gives b.
        let (mut i, mut j, mut out) = (0, 0, Vec::new());
        for o in ops {
            match o {
                Op::Same => {
                    out.push(a[i]);
                    i += 1;
                    j += 1;
                }
                Op::Del => i += 1,
                Op::Add => {
                    out.push(b[j]);
                    j += 1;
                }
            }
        }
        assert_eq!(out, b);
    }

    #[test]
    fn graph_lines_keep_running_past_other_commits() {
        // c and b on two lines that meet in a; b's line runs past c.
        let c = |h: &str, p: &[&str]| (h.to_string(), p.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let rows = graph(&[c("m", &["b", "c"]), c("c", &["x"]), c("x", &["a"]), c("b", &["a"]), c("a", &[])]);
        assert_eq!(rows[1].bottom, [(0, 0), (1, 1)]);
        assert_eq!(rows[2].top, [(0, 0), (1, 1)]);
        assert_eq!(rows[3].lane, 0);
        assert_eq!(rows[4].top, [(0, 0), (1, 0)]);
    }

    #[test]
    fn graph_of_a_merge() {
        // d merges c into b; b and c both come from a.
        let c = |h: &str, p: &[&str]| (h.to_string(), p.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let rows = graph(&[c("d", &["b", "c"]), c("c", &["a"]), c("b", &["a"]), c("a", &[])]);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[0].bottom, [(0, 0), (0, 1)]);
        assert_eq!(rows[1].lane, 1);
        assert_eq!(rows[1].top, [(0, 0), (1, 1)]);
        assert_eq!(rows[2].lane, 0);
        // a: both lines end in its dot.
        assert_eq!(rows[3].lane, 0);
        assert_eq!(rows[3].top, [(0, 0), (1, 0)]);
        assert!(rows[3].bottom.is_empty());
        let straight = graph(&[c("b", &["a"]), c("a", &[])]);
        assert_eq!(straight.iter().map(|r| r.width).collect::<Vec<_>>(), [1, 1]);
    }

    #[test]
    fn ignore_patterns_and_clone_names() {
        assert_eq!(ignore_patterns("build.log"), ["build.log", "*.log"]);
        assert_eq!(ignore_patterns(".env"), [".env"]);
        assert_eq!(clone_dir_name("https://github.com/a/b.git"), "b");
        assert_eq!(clone_dir_name("git@host:team/tool.git/"), "tool");
        assert_eq!(clone_dir_name("/srv/repos/x"), "x");
    }

    #[test]
    fn ignore_file_gets_each_pattern_once() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("sub/.gitignore");
        add_ignore(&f, "*.log").unwrap();
        add_ignore(&f, "*.log").unwrap();
        add_ignore(&f, "build").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "*.log\nbuild\n");
    }

    #[test]
    fn change_extensions() {
        let c = |p: &str| Change { path: p.into(), old_path: None, status: Status::Modified, staged: false, added: None, removed: None };
        assert_eq!(c("src/a.rs").extension(), ".rs");
        assert_eq!(c("Makefile").extension(), "");
        assert_eq!(c("dir.d/.hidden").extension(), "");
    }
}
