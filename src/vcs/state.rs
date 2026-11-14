//! The words every version-control tool shares (DDD: the domain's own language): the state a
//! file or folder is in, a change waiting to be committed, a revision of the history and its
//! labels, a line of blame, a row of a side-by-side diff and a row of the revision graph. Nothing
//! here runs a client or needs a display; each backend (`git`, `hg`, `svn`, `cvs`, `vss`) answers
//! in these words.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;

/// The state of a file or folder, as its overlay icon shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Status {
    Normal,
    Ignored,
    Unversioned,
    Added,
    Renamed,
    Deleted,
    Modified,
    Conflicted,
}

impl Status {
    /// The word TortoiseGit's lists show.
    pub fn label(self) -> &'static str {
        match self {
            Status::Normal => "Normal",
            Status::Ignored => "Ignored",
            Status::Unversioned => "Unversioned",
            Status::Added => "Added",
            Status::Renamed => "Renamed",
            Status::Deleted => "Deleted",
            Status::Modified => "Modified",
            Status::Conflicted => "Conflicted",
        }
    }
    /// Changed in a way a commit records (not unversioned, ignored or unchanged).
    pub fn is_change(self) -> bool {
        matches!(self, Status::Added | Status::Renamed | Status::Deleted | Status::Modified | Status::Conflicted)
    }
}

/// A changed file of the working tree, relative to the top of the working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    /// Where a renamed file was before.
    pub old_path: Option<String>,
    pub status: Status,
    /// Its changes are in the index already.
    pub staged: bool,
    /// Lines added and removed (None for binary files and those not known).
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

impl Change {
    /// The extension TortoiseGit shows in its own column (".rs").
    pub fn extension(&self) -> String {
        let name = self.path.rsplit('/').next().unwrap_or(&self.path);
        match name.rfind('.') {
            Some(i) if i > 0 => name[i..].to_string(),
            _ => String::new(),
        }
    }
}

/// The overlay status of the entries of the folder `rel` (relative to the top of the tree)
/// from its status lines and the tracked files under it.
pub fn folder_status_from<'a>(rel: &str, changes: &[Change], tracked: impl Iterator<Item = &'a str>) -> HashMap<OsString, Status> {
    let prefix = if rel.is_empty() { String::new() } else { format!("{rel}/") };
    let child = |path: &str| -> Option<(String, bool)> {
        let rest = path.strip_prefix(&prefix)?;
        Some(match rest.split_once('/') {
            Some((first, _)) => (first.to_string(), true),
            None => (rest.to_string(), false),
        })
    };
    let mut out: HashMap<OsString, Status> = HashMap::new();
    for t in tracked {
        if let Some((name, _)) = child(t) {
            out.entry(OsString::from(name)).or_insert(Status::Normal);
        }
    }
    for c in changes {
        let Some((name, inside)) = child(&c.path) else { continue };
        let key = OsString::from(&name);
        let st = match (c.status, inside) {
            // What lies inside a folder: changes make it modified, a conflict conflicted;
            // unversioned or ignored files inside do not change a versioned folder.
            (Status::Conflicted, true) => Status::Conflicted,
            (s, true) if s.is_change() => Status::Modified,
            (Status::Unversioned | Status::Ignored, true) => {
                out.entry(key).or_insert(c.status);
                continue;
            }
            (s, _) => s,
        };
        let e = out.entry(key).or_insert(st);
        // A deleted file shown again (re-created, untracked) stays deleted; otherwise the
        // more important state wins.
        if rank(st) > rank(*e) {
            *e = st;
        }
    }
    out
}

/// How much a state matters when one folder holds several.
fn rank(s: Status) -> u8 {
    match s {
        Status::Ignored => 0,
        Status::Unversioned => 1,
        Status::Normal => 2,
        Status::Added | Status::Renamed => 3,
        Status::Deleted => 4,
        Status::Modified => 5,
        Status::Conflicted => 6,
    }
}

/// Branches, tags and the labels shown next to the commits they point at.
#[derive(Debug, Default, Clone)]
pub struct Refs {
    pub branches: Vec<String>,
    pub remote_branches: Vec<String>,
    pub tags: Vec<String>,
    pub by_commit: HashMap<String, Vec<RefLabel>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefLabel {
    Branch(String),
    Remote(String),
    Tag(String),
    Stash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub committer: String,
    pub commit_time: i64,
    /// The whole message; its first line is the subject.
    pub message: String,
}

impl Commit {
    pub fn subject(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
    pub fn short(&self) -> &str {
        &self.hash[..self.hash.len().min(8)]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    pub hash: String,
    pub author: String,
    pub time: i64,
    pub summary: String,
    /// The line number in the file shown (from 1).
    pub line: usize,
    pub text: String,
}

/// One line of a side-by-side diff: what the left (old) and right (new) side show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub left: Option<(usize, String)>,
    pub right: Option<(usize, String)>,
    pub kind: DiffKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Same,
    /// Only on the left: removed.
    Removed,
    /// Only on the right: added.
    Added,
    /// Changed: the old line left, the new one right.
    Changed,
}

/// Lines `old` and `new` side by side, removed and added lines paired where a block of
/// them replaces another.
pub fn side_by_side(old: &str, new: &str) -> Vec<DiffRow> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let ops = diff_lines(&a, &b);
    let mut rows = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let mut k = 0;
    while k < ops.len() {
        match ops[k] {
            Op::Same => {
                rows.push(DiffRow { left: Some((i + 1, a[i].to_string())), right: Some((j + 1, b[j].to_string())), kind: DiffKind::Same });
                i += 1;
                j += 1;
                k += 1;
            }
            _ => {
                // A run of removals and additions, paired line by line.
                let (mut dels, mut adds) = (Vec::new(), Vec::new());
                while k < ops.len() && ops[k] != Op::Same {
                    if ops[k] == Op::Del {
                        dels.push(i);
                        i += 1;
                    } else {
                        adds.push(j);
                        j += 1;
                    }
                    k += 1;
                }
                for n in 0..dels.len().max(adds.len()) {
                    let l = dels.get(n).map(|&x| (x + 1, a[x].to_string()));
                    let r = adds.get(n).map(|&x| (x + 1, b[x].to_string()));
                    let kind = match (&l, &r) {
                        (Some(_), Some(_)) => DiffKind::Changed,
                        (Some(_), None) => DiffKind::Removed,
                        _ => DiffKind::Added,
                    };
                    rows.push(DiffRow { left: l, right: r, kind });
                }
            }
        }
    }
    rows
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Op {
    Same,
    Del,
    Add,
}

/// A shortest line diff (Myers) of `a` and `b`, with equal lines at both ends taken off
/// first so large files with small changes stay quick.
pub(super) fn diff_lines(a: &[&str], b: &[&str]) -> Vec<Op> {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (ma, mb) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let mut ops = vec![Op::Same; pre];
    ops.extend(myers(ma, mb));
    ops.extend(std::iter::repeat_n(Op::Same, suf));
    ops
}

fn myers(a: &[&str], b: &[&str]) -> Vec<Op> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 || m == 0 {
        return std::iter::repeat_n(Op::Del, n as usize).chain(std::iter::repeat_n(Op::Add, m as usize)).collect();
    }
    // Too big for the quadratic trace: everything replaced.
    if (n + m) as usize > 40_000 {
        return std::iter::repeat_n(Op::Del, n as usize).chain(std::iter::repeat_n(Op::Add, m as usize)).collect();
    }
    let max = n + m;
    let off = max;
    let mut v = vec![0isize; (2 * max + 2) as usize];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    'outer: for d in 0..=max {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let idx = (k + off) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) { v[idx + 1] } else { v[idx - 1] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x >= n && y >= m {
                trace.push(v.clone());
                break 'outer;
            }
            k += 2;
        }
    }
    // Walk back through the trace.
    let mut ops = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..trace.len() - 1).rev() {
        let v = &trace[d];
        let d = d as isize;
        let k = x - y;
        let idx = (k + off) as usize;
        let prev_k = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) { k + 1 } else { k - 1 };
        let prev_x = v[(prev_k + off) as usize];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            ops.push(Op::Same);
            x -= 1;
            y -= 1;
        }
        if x == prev_x {
            ops.push(Op::Add);
        } else {
            ops.push(Op::Del);
        }
        x = prev_x;
        y = prev_y;
    }
    while x > 0 && y > 0 {
        ops.push(Op::Same);
        x -= 1;
        y -= 1;
    }
    ops.reverse();
    ops
}

/// Where a commit sits in the revision graph and which lines pass its row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    /// The column of the commit's dot.
    pub lane: usize,
    /// Lines from the row above: (column above, column here).
    pub top: Vec<(usize, usize)>,
    /// Lines to the row below: (column here, column below).
    pub bottom: Vec<(usize, usize)>,
    /// Columns in use in this row.
    pub width: usize,
}

/// Lays out the graph of `commits` (newest first, each with its parents) in columns.
pub fn graph(commits: &[(String, Vec<String>)]) -> Vec<GraphRow> {
    // lanes[i]: the commit the line in column i is heading for.
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    for (hash, parents) in commits {
        let before = lanes.clone();
        let lane = match lanes.iter().position(|l| l.as_deref() == Some(hash)) {
            Some(i) => i,
            None => match lanes.iter().position(Option::is_none) {
                Some(i) => {
                    lanes[i] = Some(hash.clone());
                    i
                }
                None => {
                    lanes.push(Some(hash.clone()));
                    lanes.len() - 1
                }
            },
        };
        // Lines from above: every column that was heading somewhere continues; those
        // heading for this commit end in its dot.
        let mut top = Vec::new();
        for (i, l) in before.iter().enumerate() {
            match l {
                Some(h) if h == hash => top.push((i, lane)),
                Some(_) => top.push((i, i)),
                None => {}
            }
        }
        // Other columns that also waited for this commit are free now.
        for (i, l) in lanes.iter_mut().enumerate() {
            if i != lane && l.as_deref() == Some(hash) {
                *l = None;
            }
        }
        // The first parent takes over the commit's column; others get columns of their own.
        lanes[lane] = parents.first().cloned();
        let mut bottom = Vec::new();
        if !parents.is_empty() {
            bottom.push((lane, lane));
        }
        for p in parents.iter().skip(1) {
            let target = match lanes.iter().position(|l| l.as_deref() == Some(p)) {
                Some(i) => i,
                None => match lanes.iter().position(Option::is_none) {
                    Some(i) => {
                        lanes[i] = Some(p.clone());
                        i
                    }
                    None => {
                        lanes.push(Some(p.clone()));
                        lanes.len() - 1
                    }
                },
            };
            bottom.push((lane, target));
        }
        // Every other column in use goes on straight down, unless this row only just opened
        // it for one of its parents (then its line comes from the dot).
        for (i, l) in lanes.iter().enumerate() {
            if i == lane || l.is_none() {
                continue;
            }
            let ran_on = before.get(i).is_some_and(|b| b.as_ref().is_some_and(|h| h != hash));
            if ran_on || !bottom.iter().any(|&(f, t)| f == lane && t == i) {
                bottom.push((i, i));
            }
        }
        bottom.sort();
        bottom.dedup();
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        let width = top.iter().chain(&bottom).flat_map(|&(a, b)| [a, b]).chain([lane]).max().unwrap_or(0) + 1;
        rows.push(GraphRow { lane, top, bottom, width });
    }
    rows
}

/// Adds `pattern` to an ignore file (creating it), unless it is there already.
pub fn add_ignore(file: &Path, pattern: &str) -> std::io::Result<()> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    if text.lines().any(|l| l.trim() == pattern) {
        return Ok(());
    }
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut new = text;
    if !new.is_empty() && !new.ends_with('\n') {
        new.push('\n');
    }
    new.push_str(pattern);
    new.push('\n');
    std::fs::write(file, new)
}

/// The patterns TortoiseGit's "Add to ignore list" offers for a file: its name, and
/// "*.ext" for its extension.
pub fn ignore_patterns(name: &str) -> Vec<String> {
    let mut out = vec![name.to_string()];
    if let Some(i) = name.rfind('.').filter(|&i| i > 0) {
        out.push(format!("*{}", &name[i..]));
    }
    out
}


/// Lines added and removed per file in a unified diff, keyed by the path the `+++` line names
/// (the `---` one for a deleted file), with `strip` taken off its front (`b/` for Mercurial) and
/// what follows a tab (a date, "(working copy)") left out. What Git's `--numstat` answers, for the
/// clients that only print the diff itself.
pub fn count_unified(text: &str, strip: &str) -> HashMap<String, (Option<u64>, Option<u64>)> {
    let name = |s: &str, prefix: &str| -> String {
        let s = s.split('\t').next().unwrap_or(s).trim_end();
        s.strip_prefix(prefix).unwrap_or(s).to_string()
    };
    let mut out: HashMap<String, (Option<u64>, Option<u64>)> = HashMap::new();
    let mut old = String::new();
    let mut current: Option<String> = None;
    let mut in_hunk = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("--- ").filter(|_| !in_hunk || current.is_none()) {
            old = rest.to_string();
            continue;
        }
        if let Some(rest) = line.strip_prefix("+++ ").filter(|_| !old.is_empty()) {
            let new = rest.split('\t').next().unwrap_or(rest);
            let a_side = if strip == "b/" { "a/" } else { strip };
            let path = if new == "/dev/null" { name(&old, a_side) } else { name(rest, strip) };
            out.entry(path.clone()).or_insert((Some(0), Some(0)));
            current = Some(path);
            old.clear();
            in_hunk = false;
            continue;
        }
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            continue;
        }
        let Some(path) = &current else { continue };
        let e = out.get_mut(path).expect("entry made at its header");
        if line.starts_with('+') {
            e.0 = e.0.map(|n| n + 1);
        } else if line.starts_with('-') {
            e.1 = e.1.map(|n| n + 1);
        } else if !line.starts_with(' ') && !line.starts_with('\\') {
            // A line outside the hunk's alphabet starts the next file's header.
            in_hunk = false;
        }
    }
    out
}

#[cfg(test)]
mod unified_tests {
    use super::*;

    #[test]
    fn lines_are_counted_per_file_of_a_mercurial_diff() {
        let text = "diff -r 1 one.txt\n--- a/one.txt\tThu Jan 01 00:00:00 1970 +0000\n+++ b/one.txt\tThu Jan 01 00:00:00 1970 +0000\n@@ -1,2 +1,3 @@\n alpha\n-beta\n+beta changed\n+gamma\ndiff -r 1 gone.txt\n--- a/gone.txt\tThu\n+++ /dev/null\tThu\n@@ -1,1 +0,0 @@\n-x\n";
        let c = count_unified(text, "b/");
        assert_eq!(c["one.txt"], (Some(2), Some(1)));
        assert_eq!(c["gone.txt"], (Some(0), Some(1)));
    }

    #[test]
    fn a_subversion_diff_names_its_files_without_a_prefix() {
        let text = "Index: one.txt\n===================================================================\n--- one.txt\t(revision 2)\n+++ one.txt\t(working copy)\n@@ -1 +1 @@\n-a\n+b\n";
        assert_eq!(count_unified(text, "")["one.txt"], (Some(1), Some(1)));
    }

    /// A removed line reading `-- x` must not be taken for the next file's `---` header.
    #[test]
    fn a_removed_line_that_looks_like_a_header_stays_a_line() {
        let text = "--- a/f\n+++ b/f\n@@ -1,2 +1,1 @@\n--- x\n keep\n";
        assert_eq!(count_unified(text, "b/")["f"], (Some(0), Some(1)));
    }
}

/// Lines of a file not yet in the repository: every line counts as added (None for binary files
/// and those that cannot be read).
pub fn new_file_lines(path: &Path) -> (Option<u64>, Option<u64>) {
    match std::fs::read(path) {
        Ok(text) if !text.contains(&0) => (Some(text.iter().filter(|&&b| b == b'\n').count() as u64 + u64::from(!text.is_empty() && !text.ends_with(b"\n"))), Some(0)),
        _ => (None, None),
    }
}

/// What the diff window shows at one position: a row of the side-by-side diff, or a run of
/// unchanged rows folded away, named by the lines it hides on each side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    Row(usize),
    Fold { rows: std::ops::Range<usize>, old: (usize, usize), new: (usize, usize) },
}

/// The rows with each unchanged run longer than it needs to be folded down to `context` rows on
/// each side of a change (at the start and end of the file, on the change's side only). A run
/// that would fold a single row stays shown: naming one hidden line costs as much as showing it.
pub fn fold_same(rows: &[DiffRow], context: usize) -> Vec<Shown> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if rows[i].kind != DiffKind::Same {
            out.push(Shown::Row(i));
            i += 1;
            continue;
        }
        let start = i;
        while i < rows.len() && rows[i].kind == DiffKind::Same {
            i += 1;
        }
        let end = i;
        let keep_before = if start == 0 { 0 } else { context };
        let keep_after = if end == rows.len() { 0 } else { context };
        if end - start <= keep_before + keep_after + 1 {
            out.extend((start..end).map(Shown::Row));
            continue;
        }
        out.extend((start..start + keep_before).map(Shown::Row));
        let hidden = start + keep_before..end - keep_after;
        let line = |r: &DiffRow, left: bool| (if left { &r.left } else { &r.right }).as_ref().map(|(n, _)| *n).unwrap_or(0);
        let (first, last) = (&rows[hidden.start], &rows[hidden.end - 1]);
        out.push(Shown::Fold { old: (line(first, true), line(last, true)), new: (line(first, false), line(last, false)), rows: hidden.clone() });
        out.extend((hidden.end..end).map(Shown::Row));
    }
    out
}

#[cfg(test)]
mod fold_tests {
    use super::*;

    fn text(n: usize, changed_at: &[usize]) -> (String, String) {
        let old: Vec<String> = (1..=n).map(|i| format!("line {i}")).collect();
        let new: Vec<String> = (1..=n).map(|i| if changed_at.contains(&i) { format!("line {i} changed") } else { format!("line {i}") }).collect();
        (old.join("\n") + "\n", new.join("\n") + "\n")
    }

    #[test]
    fn unchanged_runs_fold_to_three_lines_around_a_change() {
        let (a, b) = text(30, &[15]);
        let rows = side_by_side(&a, &b);
        let shown = fold_same(&rows, 3);
        let folds: Vec<_> = shown.iter().filter_map(|s| if let Shown::Fold { old, .. } = s { Some(*old) } else { None }).collect();
        assert_eq!(folds, [(1, 11), (19, 30)], "the start up to three lines before, from three lines after to the end");
        let visible = shown.iter().filter(|s| matches!(s, Shown::Row(_))).count();
        assert_eq!(visible, 3 + 1 + 3);
    }

    #[test]
    fn a_short_run_between_two_changes_stays_whole() {
        let (a, b) = text(12, &[3, 10]);
        let rows = side_by_side(&a, &b);
        let shown = fold_same(&rows, 3);
        // Between 3 and 10 lie six unchanged lines: three after the first change, three before the
        // second, nothing left to fold.
        assert!(shown.iter().all(|s| !matches!(s, Shown::Fold { old: (4, _), .. })), "{shown:?}");
    }

    #[test]
    fn one_line_is_never_folded() {
        let (a, b) = text(8, &[1]);
        let rows = side_by_side(&a, &b);
        // Seven unchanged lines after the change: three kept, four folded.
        let shown = fold_same(&rows, 3);
        assert!(shown.contains(&Shown::Fold { rows: 4..8, old: (5, 8), new: (5, 8) }), "{shown:?}");
        let (a, b) = text(5, &[1]);
        assert!(fold_same(&side_by_side(&a, &b), 3).iter().all(|s| matches!(s, Shown::Row(_))), "four lines after: one would fold");
    }

    #[test]
    fn a_file_without_changes_folds_to_one_row() {
        let (a, _) = text(10, &[]);
        assert_eq!(fold_same(&side_by_side(&a, &a), 3), [Shown::Fold { rows: 0..10, old: (1, 10), new: (1, 10) }]);
    }
}
