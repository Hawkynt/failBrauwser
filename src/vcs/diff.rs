//! The shape of a diff a version-control client reports (M15). The client's own unified output is
//! the only thing read here: it already says which lines were added, removed and left alone, and
//! the window shows exactly that — nothing here opens two files and finds their differences again,
//! because then the window would disagree with what the tool's own client told the user.
//!
//! The rows are built so the two sides share one row per position (ShiftDiff's aligned-rows idea,
//! learned from its behaviour under LGPL-3.0-or-later and re-implemented here with its own naming
//! and for a client's output rather than for two whole files), and inside a changed pair the words
//! that actually changed are marked apart from the words that only sit next to them.

/// A line's place in a diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The header naming the file the lines below it belong to.
    File,
    Context,
    Added,
    Removed,
    /// Both sides have a line here and they differ.
    Changed,
    /// Lines the client did not report between two of its hunks.
    Fold,
}

/// A run of characters inside a row's text that did (or did not) change. A row of one unchanged
/// run is what the two sides agree on; a row of several says which words moved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    pub changed: bool,
    pub text: String,
}

/// One row of the diff window: what each side shows at this position, and the row's own place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub kind: Kind,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub old: Vec<Mark>,
    pub new: Vec<Mark>,
}

impl Row {
    /// What one side shows, marks joined.
    pub fn text(&self, side: Side) -> String {
        self.marks(side).iter().map(|m| m.text.as_str()).collect()
    }

    pub fn marks(&self, side: Side) -> &[Mark] {
        match side {
            Side::Old => &self.old,
            Side::New => &self.new,
        }
    }

    /// Whether this row is one of the changes *next change* and *previous change* walk between.
    pub fn is_change(&self) -> bool {
        matches!(self.kind, Kind::Added | Kind::Removed | Kind::Changed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Old,
    New,
}

/// What a diff adds up to, for the window's status line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub files: usize,
    pub added: usize,
    pub removed: usize,
}

/// The rows a window shows for one client's unified diff, with unchanged runs folded to `context`
/// rows each and the range the client left out named.
pub fn rows(unified: &str, context: usize) -> Vec<Row> {
    fold(&changes(unified), context)
}

/// The rows of one client's diff, unfolded: every line the client reported, paired across the two
/// sides inside each change.
pub fn changes(unified: &str) -> Vec<Row> {
    let mut p = Parser::default();
    p.read(unified);
    p.rows
}

/// The shape of a client's answer, read line by line. What a unified diff gives is a file header,
/// hunk headers saying where the reported lines sit, and the lines themselves.
#[derive(Default)]
struct Parser {
    rows: Vec<Row>,
    old: Option<String>,
    new: Option<String>,
    /// The `File` row of the file being read is already in `rows`.
    file_row: bool,
    /// A hunk header or a `---`/`+++` pair has been seen for this file, so what follows without a
    /// marker of its own is one of its lines.
    body: bool,
    /// The client writes its file names under `a/` and `b/`, which only Git does.
    git_style: bool,
    /// A `---` line has been read for the file being read, so the next one starts another file:
    /// CVS and Subversion name their files that way, with no `diff --git` line to go by.
    names_started: bool,
    /// Inside a hunk, and how many lines of it the client still has to report.
    hunk: bool,
    old_left: usize,
    new_left: usize,
    old_at: usize,
    new_at: usize,
    /// The highest line number of the old side reported for this file so far, which answers what
    /// the client said nothing about between two of its hunks.
    last_old: Option<usize>,
    removed: Vec<(usize, String)>,
    added: Vec<(usize, String)>,
}

impl Parser {
    fn read(&mut self, unified: &str) {
        // `lines` rather than a split: an answer ending in a newline has no empty line after it,
        // and a client on Windows ends its lines with a carriage return the window does not show.
        for line in unified.lines() {
            if self.hunk && (self.old_left > 0 || self.new_left > 0) {
                self.reported(line);
                continue;
            }
            self.hunk = false;
            self.between(line);
        }
        self.finish();
    }

    /// A line the client reported as part of the current hunk, which owes the hunk one of the line
    /// counts its header announced.
    fn reported(&mut self, line: &str) {
        match line.chars().next() {
            Some('-') => self.old_left -= 1,
            Some('+') => self.new_left -= 1,
            Some('\\') => return, // `\ No newline at end of file`: no line of either side
            _ => {
                self.old_left -= 1;
                self.new_left -= 1;
            }
        }
        self.line(line);
    }

    /// One reported line: one the client says was removed, one it says was added, and one it says
    /// both sides have. Kept runs are broken up here, so a removal and the addition beside it are
    /// read as one edited line rather than as two unrelated ones.
    fn line(&mut self, line: &str) {
        match line.chars().next() {
            Some('-') => {
                self.old_at += 1;
                self.removed.push((self.old_at, line[1..].to_string()));
            }
            Some('+') => {
                self.new_at += 1;
                self.added.push((self.new_at, line[1..].to_string()));
            }
            _ => self.context(line.strip_prefix(' ').unwrap_or(line).to_string()),
        }
    }

    /// A line outside a hunk: a file header, a hunk header, or — for the client that reported no
    /// hunk header at all — one of the file's own lines.
    fn between(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            self.finish();
            self.git_style = true;
            if let Some((old, new)) = git_names(rest) {
                self.old = Some(old);
                self.new = Some(new);
            }
            return;
        }
        // Subversion and CVS name the file they are about in a line of their own, and neither
        // writes a `diff --git` line: this is where one file of their answer ends.
        if let Some(rest) = line.strip_prefix("Index:") {
            self.finish();
            self.old = named(rest.trim_start(), false);
            self.new = self.old.clone();
            return;
        }
        if line.starts_with("RCS file:") || line.starts_with("diff ") || line.starts_with("similarity ") || line.starts_with("deleted file mode") || line.starts_with("new file mode") || line.starts_with("old mode") || line.starts_with("new mode") || line.starts_with("index ") {
            if line.starts_with("RCS file:") {
                self.finish();
            }
            return;
        }
        if let Some(name) = line.strip_prefix("rename from ") {
            self.old = Some(name.to_string());
            return;
        }
        if let Some(name) = line.strip_prefix("rename to ") {
            self.new = Some(name.to_string());
            return;
        }
        if let Some(name) = line.strip_prefix("--- ") {
            if self.names_started {
                self.finish();
            }
            self.names_started = true;
            self.body = true;
            self.old = named(name, self.git_style);
            return;
        }
        if let Some(name) = line.strip_prefix("+++ ") {
            self.body = true;
            self.new = named(name, self.git_style);
            return;
        }
        if line.starts_with("@@") {
            self.open();
            self.body = true;
            self.flush();
            if let Some((old_start, old_count, new_start, new_count)) = hunk(line) {
                self.note_hidden(old_start);
                self.hunk = true;
                self.old_left = old_count;
                self.new_left = new_count;
                self.old_at = old_start.saturating_sub(1);
                self.new_at = new_start.saturating_sub(1);
            }
            return;
        }
        // A client that gives no hunk header: once its file is open, an unmarked line is one of
        // the file's own lines, and the window still has to show it.
        if !self.body {
            return;
        }
        self.open();
        self.line(line);
    }

    /// The `File` row of the file being read, once its name is known.
    fn open(&mut self) {
        if self.file_row || (self.old.is_none() && self.new.is_none()) {
            return;
        }
        self.file_row = true;
        self.rows.push(Row { kind: Kind::File, old_line: None, new_line: None, old: plain(self.old.clone().unwrap_or_default()), new: plain(self.new.clone().unwrap_or_default()) });
    }

    fn context(&mut self, text: String) {
        self.flush();
        self.old_at += 1;
        self.new_at += 1;
        self.last_old = Some(self.old_at);
        self.rows.push(Row { kind: Kind::Context, old_line: Some(self.old_at), new_line: Some(self.new_at), old: plain(text.clone()), new: plain(text) });
    }

    /// What the client said nothing about between the last line it reported and this hunk.
    fn note_hidden(&mut self, next_old: usize) {
        let hidden = match self.last_old {
            Some(prev) if next_old > prev + 1 => next_old - prev - 1,
            _ => return,
        };
        self.rows.push(Row { kind: Kind::Fold, old_line: Some(self.last_old.unwrap_or(0) + 1), new_line: Some(self.new_at + 1), old: plain(format!("{hidden} rows")), new: plain(format!("{hidden} rows")) });
    }

    /// The lines the client reported between two context lines or two files: the ones it says were
    /// removed and the ones it says were added, one row per position across the two sides. A block
    /// of three removals beside two additions is therefore two rows the sides share and one row
    /// the old side has alone.
    fn flush(&mut self) {
        let removed = std::mem::take(&mut self.removed);
        let added = std::mem::take(&mut self.added);
        for at in 0..removed.len().max(added.len()) {
            match (removed.get(at), added.get(at)) {
                (Some((old_line, old_text)), Some((new_line, new_text))) => {
                    let (old_marks, new_marks) = words(old_text, new_text);
                    self.last_old = Some(*old_line);
                    self.rows.push(Row { kind: Kind::Changed, old_line: Some(*old_line), new_line: Some(*new_line), old: old_marks, new: new_marks });
                }
                (Some((old_line, old_text)), None) => {
                    self.last_old = Some(*old_line);
                    self.rows.push(Row { kind: Kind::Removed, old_line: Some(*old_line), new_line: None, old: plain(old_text.clone()), new: Vec::new() });
                }
                (None, Some((new_line, new_text))) => self.rows.push(Row { kind: Kind::Added, old_line: None, new_line: Some(*new_line), old: Vec::new(), new: plain(new_text.clone()) }),
                (None, None) => {}
            }
        }
    }

    /// The end of one file's answer: whatever was still reported, and a file the client named
    /// without reporting a line of it at all — a rename, which moves no line.
    fn finish(&mut self) {
        self.flush();
        self.open();
        self.old = None;
        self.new = None;
        self.file_row = false;
        self.body = false;
        self.names_started = false;
        self.git_style = false;
        self.hunk = false;
        self.old_left = 0;
        self.new_left = 0;
        self.last_old = None;
        self.old_at = 0;
        self.new_at = 0;
    }
}

/// The rows a window shows when unchanged runs between changes are folded: `context` rows of each
/// run, and a fold row naming how many lines the client left out.
pub fn fold(rows: &[Row], context: usize) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::with_capacity(rows.len());
    let mut at = 0;
    while at < rows.len() {
        if rows[at].kind != Kind::Context {
            out.push(rows[at].clone());
            at += 1;
            continue;
        }
        let start = at;
        while at < rows.len() && rows[at].kind == Kind::Context {
            at += 1;
        }
        let run = &rows[start..at];
        // A run is only worth a row of its own once folding hides something a reader would miss.
        if run.len() <= context * 2 + 1 {
            out.extend(run.iter().cloned());
            continue;
        }
        out.extend(run[..context].iter().cloned());
        let hidden = run[context].clone();
        out.push(Row { kind: Kind::Fold, old_line: hidden.old_line, new_line: hidden.new_line, old: plain(format!("{} rows", run.len() - context * 2)), new: plain(format!("{} rows", run.len() - context * 2)) });
        out.extend(run[run.len() - context..].iter().cloned());
    }
    out
}

/// The counts of one client's diff, for the window's status line.
pub fn summary(unified: &str) -> Summary {
    let mut out = Summary::default();
    for row in changes(unified) {
        match row.kind {
            Kind::File => out.files += 1,
            Kind::Added => out.added += 1,
            Kind::Removed => out.removed += 1,
            Kind::Changed => {
                out.added += 1;
                out.removed += 1;
            }
            _ => {}
        }
    }
    out
}

/// A line of text with nothing marked in it: the whole of it is one run.
fn plain(text: String) -> Vec<Mark> {
    if text.is_empty() {
        return Vec::new();
    }
    vec![Mark { changed: false, text }]
}

/// A file name as the client writes it: Git keeps the old side under `a/` and the new under `b/`,
/// and a side with no file at all is `/dev/null`. CVS puts a date after a tab.
fn named(raw: &str, git_style: bool) -> Option<String> {
    let bare = raw.split('\t').next().unwrap_or(raw).trim();
    if bare == "/dev/null" || bare.is_empty() {
        return None;
    }
    if git_style {
        // A prefix standing alone names no file, and the bare name is the honest answer then.
        if let Some(rest) = bare.strip_prefix("a/").or_else(|| bare.strip_prefix("b/")).filter(|r| !r.is_empty()) {
            return Some(rest.to_string());
        }
    }
    Some(bare.to_string())
}

/// The four numbers of a hunk header: where each side starts and how many lines the client
/// reports there. `@@ -1,7 +1,9 @@` says the first; a missing count is one line, and the counts of
/// a hunk can be left out entirely for one line (`@@ -1 +1 @@`).
fn hunk(line: &str) -> Option<(usize, usize, usize, usize)> {
    let bounds = line.strip_prefix("@@")?.split('@').next()?.trim();
    let (old, new) = bounds.split_once('+')?;
    let side = |s: &str| -> Option<(usize, usize)> {
        let head = s.trim().trim_start_matches(['-', '+']).split_whitespace().next()?;
        let (start, count) = match head.split_once(',') {
            Some((a, b)) => (a, b),
            None => (head, "1"),
        };
        Some((start.parse().ok()?, count.parse().ok()?))
    };
    let (old_start, old_count) = side(old)?;
    let (new_start, new_count) = side(new)?;
    Some((old_start, old_count, new_start, new_count))
}

/// The two names of a `diff --git a/x b/y` line, which is how Git names a file whose own header
/// lines may never arrive (a rename, or a mode change).
fn git_names(pair: &str) -> Option<(String, String)> {
    let mid = pair.find(" b/")?;
    let old = named(pair.get(..mid)?, true);
    let new = named(pair.get(mid + 2..)?, true);
    Some((old?, new?))
}

/// The rows *next change* and *previous change* walk, in row order.
pub fn change_rows(rows: &[Row]) -> Vec<usize> {
    rows.iter().enumerate().filter(|(_, r)| r.is_change()).map(|(i, _)| i).collect()
}

/// The words a changed pair has in common and the words between them, for one side of the pair.
/// A token is a word with the blank that stands before it, so the marks of a side join back into
/// the line they came from and a window can style a row without rewriting a character of it.
pub fn words(before: &str, after: &str) -> (Vec<Mark>, Vec<Mark>) {
    let (a, b) = (tokens(before), tokens(after));
    if a.len() > INLINE_TOKENS || b.len() > INLINE_TOKENS {
        return (vec![Mark { changed: true, text: before.to_string() }], vec![Mark { changed: true, text: after.to_string() }]);
    }
    let (old_flags, new_flags) = in_step(&a, &b);
    (runs(&a, &old_flags), runs(&b, &new_flags))
}

/// How many tokens one side of a pair may have before its words stop being lined up word by word.
/// The pairing is a table of both sides, so a minified line of ten thousand words would ask for a
/// hundred million cells to mark a change nobody reads word by word anyway.
const INLINE_TOKENS: usize = 1_500;

/// Which tokens of each side stand in the same order on both sides — the words that stayed put
/// while their neighbours moved.
fn in_step(a: &[String], b: &[String]) -> (Vec<bool>, Vec<bool>) {
    // The longest common run of tokens, read back out of the usual table: `next` counts how many
    // tokens end at each pair of positions.
    let mut next = vec![vec![0u16; b.len() + 1]; a.len() + 1];
    for (i, x) in a.iter().enumerate() {
        for (j, y) in b.iter().enumerate() {
            next[i + 1][j + 1] = if x == y { next[i][j] + 1 } else { next[i][j + 1].max(next[i + 1][j]) };
        }
    }
    let (mut old, mut new) = (vec![false; a.len()], vec![false; b.len()]);
    let (mut i, mut j) = (a.len(), b.len());
    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            (old[i - 1], new[j - 1]) = (true, true);
            (i, j) = (i - 1, j - 1);
        } else if next[i - 1][j] >= next[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    (old, new)
}

/// Neighbours that stand or fall together become one mark, which is what lets a window show the
/// changed run of a row as a single style.
fn runs(tokens: &[String], in_step: &[bool]) -> Vec<Mark> {
    let mut out: Vec<Mark> = Vec::new();
    for (token, &same) in tokens.iter().zip(in_step) {
        let changed = !same;
        match out.last_mut() {
            Some(last) if last.changed == changed => last.text.push_str(token),
            _ => out.push(Mark { changed, text: token.clone() }),
        }
    }
    out
}

/// The tokens of a line: each word with the blank run before it, the first word alone.
fn tokens(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut rest = line;
    while !rest.is_empty() {
        let (blank, tail) = split(rest, rest.chars().take_while(|c| c.is_whitespace()).count());
        let (word, tail) = split(tail, tail.chars().take_while(|c| !c.is_whitespace()).count());
        if !blank.is_empty() && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current.push_str(blank);
        current.push_str(word);
        rest = tail;
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// A prefix of `upto` characters of `s` and what follows, by char count rather than by bytes.
fn split(s: &str, upto: usize) -> (&str, &str) {
    let byte = s.char_indices().nth(upto).map(|(i, _)| i).unwrap_or(s.len());
    s.split_at(byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The unified diff `git diff` answers for one edited file, kept as the fixture every scenario
    /// here reads: the shape all five clients agree on.
    const EDITED: &str = "diff --git a/notes.txt b/notes.txt\n\
                           index 3f4a1b2..9c8d7e6 100644\n\
                           --- a/notes.txt\n\
                           +++ b/notes.txt\n\
                           @@ -1,3 +1,3 @@\n\
                            one\n\
                           -two old line\n\
                           +two new line here\n\
                            three\n";

    #[test]
    fn a_client_s_diff_names_the_file_it_reports() {
        let rows = changes(EDITED);
        let file = rows.iter().find(|r| r.kind == Kind::File).expect("the diff has to name its file");
        assert_eq!(file.text(Side::New), "notes.txt");
        assert_eq!(file.text(Side::Old), "notes.txt");
    }

    #[test]
    fn an_edited_line_shares_one_row_between_the_two_sides() {
        let rows = changes(EDITED);
        let changed = rows.iter().filter(|r| r.kind == Kind::Changed).collect::<Vec<_>>();
        assert_eq!(changed.len(), 1, "one line was edited, so one row carries both sides: {rows:?}");
        assert_eq!(changed[0].old_line, Some(2), "the old side knows its line number");
        assert_eq!(changed[0].new_line, Some(2), "and so does the new side at the same row");
        assert_eq!(changed[0].text(Side::Old), "two old line");
        assert_eq!(changed[0].text(Side::New), "two new line here");
    }

    #[test]
    fn the_words_that_changed_stand_out_from_the_words_they_sit_between() {
        let rows = changes(EDITED);
        let changed = rows.iter().find(|r| r.kind == Kind::Changed).expect("the edited line");
        let kept = |side: Side| changed.marks(side).iter().filter(|m| !m.changed).map(|m| m.text.as_str()).collect::<String>();
        let moved = |side: Side| changed.marks(side).iter().filter(|m| m.changed).map(|m| m.text.as_str()).collect::<String>();
        // "two" and "line" are common to both sides and stand unmarked; "old" left and "new here"
        // arrived. The marks join back into the line they came from, so a window styles a row
        // without rewriting a single character of it.
        assert_eq!(kept(Side::Old), kept(Side::New), "the words both sides keep read the same on each of them: {:?} against {:?}", changed.marks(Side::Old), changed.marks(Side::New));
        assert!(kept(Side::Old).contains("two") && kept(Side::Old).contains("line"), "the words that stayed put: {:?}", changed.marks(Side::Old));
        assert_eq!(moved(Side::Old), " old", "the word that left: {:?}", changed.marks(Side::Old));
        assert!(moved(Side::New).contains("new") && moved(Side::New).contains("here"), "and the words that arrived: {:?}", changed.marks(Side::New));
        assert_eq!(changed.text(Side::Old), "two old line");
        assert_eq!(changed.text(Side::New), "two new line here");
    }

    /// A context line stands for both sides: the window shows it once, with both line numbers.
    #[test]
    fn a_line_both_sides_agree_on_is_one_row_with_both_numbers() {
        let rows = changes(EDITED);
        let context = rows.iter().filter(|r| r.kind == Kind::Context).collect::<Vec<_>>();
        assert_eq!(context.len(), 2, "the client reported two lines of context: {rows:?}");
        assert_eq!((context[0].old_line, context[0].new_line), (Some(1), Some(1)));
        assert_eq!((context[1].old_line, context[1].new_line), (Some(3), Some(3)));
        assert_eq!(context[0].text(Side::Old), "one");
        assert_eq!(context[0].text(Side::New), "one");
    }

    /// A line only one side has stands alone, and the other side has nothing at that row.
    #[test]
    fn a_line_only_one_side_has_leaves_the_other_side_empty() {
        let added = changes("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,1 +1,2 @@\n one\n+two\n");
        let row = added.iter().find(|r| r.kind == Kind::Added).expect("an added line");
        assert_eq!(row.new_line, Some(2));
        assert_eq!(row.old_line, None, "the old side has no line here");
        assert!(row.old.is_empty(), "and shows nothing at that row: {:?}", row.old);
        assert_eq!(row.text(Side::New), "two");
    }

    /// Two files in one client answer stay two files.
    #[test]
    fn the_files_of_one_client_answer_are_kept_apart() {
        let two = format!("{EDITED}diff --git a/other.txt b/other.txt\n--- a/other.txt\n+++ b/other.txt\n@@ -1,1 +1,0 @@\n-gone\n");
        let rows = changes(&two);
        let files = rows.iter().filter(|r| r.kind == Kind::File).collect::<Vec<_>>();
        assert_eq!(files.iter().map(|f| f.text(Side::New)).collect::<Vec<_>>(), ["notes.txt", "other.txt"]);
        let removed = rows.iter().filter(|r| r.kind == Kind::Removed).collect::<Vec<_>>();
        assert_eq!(removed.len(), 1, "the second file's line is removed on the old side: {rows:?}");
        assert_eq!(removed[0].old_line, Some(1));
        assert_eq!(removed[0].new_line, None);
    }

    /// A rename is one change whose two sides carry different names.
    #[test]
    fn a_rename_shows_its_two_names() {
        let renamed = "diff --git a/old.txt b/new.txt\nsimilarity index 100%\nrename from old.txt\nrename to new.txt\n";
        let rows = changes(renamed);
        let file = rows.iter().find(|r| r.kind == Kind::File).expect("a rename names its file");
        assert_eq!(file.text(Side::Old), "old.txt");
        assert_eq!(file.text(Side::New), "new.txt");
        assert_eq!(rows.iter().filter(|r| r.is_change()).count(), 0, "a rename moves no lines: {rows:?}");
    }

    /// The window folds a long unchanged run and names what it hides, so the user can tell a fold
    /// from the client's own silence.
    #[test]
    fn an_unchanged_run_longer_than_the_context_is_folded_and_the_hidden_range_named() {
        // Ten context rows between two changes: 3 stay at each end and four are folded away.
        let mut text = String::from("@@ -1,12 +1,12 @@\n");
        for i in 1..=10 {
            text.push_str(&format!(" line{i}\n"));
        }
        text.push_str("-old tail\n+new tail\n");
        let wide = fold(&changes(&text), 3);
        let folds = wide.iter().filter(|r| r.kind == Kind::Fold).collect::<Vec<_>>();
        assert_eq!(folds.len(), 1, "the middle of the run is folded into one row: {wide:?}");
        assert_eq!(folds[0].old_line, Some(4), "the fold starts after the rows that stayed");
        assert_eq!(folds[0].text(Side::Old), "4 rows", "and says how many it hides: {:?}", folds[0].text(Side::Old));
        let kept = wide.iter().filter(|r| r.kind == Kind::Context).count();
        assert_eq!(kept, 6, "three context rows at each end of the run stay visible: {wide:?}");
    }

    /// A client that reported no more context than three leaves nothing to fold.
    #[test]
    fn a_run_shorter_than_the_context_is_left_as_the_client_reported_it() {
        let wide = fold(&changes(EDITED), 3);
        assert!(wide.iter().all(|r| r.kind != Kind::Fold), "three context rows are already the context: {wide:?}");
    }

    /// The lines between two hunks were never reported at all; a fold row says so and names the
    /// range, which is how a folder's diff over a long file stays readable.
    #[test]
    fn what_lies_between_two_hunks_of_one_file_is_named_as_hidden() {
        let two = "@@ -1,2 +1,2 @@\n one\n-old\n+new\n@@ -40,2 +40,2 @@\n forty\n-old40\n+new40\n";
        let wide = fold(&changes(two), 3);
        let folds = wide.iter().filter(|r| r.kind == Kind::Fold).collect::<Vec<_>>();
        assert_eq!(folds.len(), 1, "one hidden range between the hunks: {wide:?}");
        assert_eq!(folds[0].text(Side::Old), "37 rows", "the rows the client said nothing about: {:?}", folds[0].text(Side::Old));
        assert_eq!(folds[0].old_line, Some(3), "and where they start");
    }

    #[test]
    fn the_counts_of_one_client_answer_add_up_for_the_status_line() {
        let two = format!("{EDITED}diff --git a/other.txt b/other.txt\n--- a/other.txt\n+++ b/other.txt\n@@ -1,1 +1,0 @@\n-gone\n");
        assert_eq!(summary(&two), Summary { files: 2, added: 1, removed: 2 });
        assert_eq!(summary(EDITED), Summary { files: 1, added: 1, removed: 1 });
        assert_eq!(summary(""), Summary::default());
    }

    #[test]
    fn the_rows_next_change_and_previous_change_walk_are_the_changes_in_order() {
        let two = format!("{EDITED}diff --git a/other.txt b/other.txt\n--- a/other.txt\n+++ b/other.txt\n@@ -1,1 +1,0 @@\n-gone\n");
        let wide = rows(&two, 3);
        let at = change_rows(&wide);
        assert_eq!(at.len(), 2, "one edited line in the first file and one removed line in the second: {wide:?}");
        assert!(at.windows(2).all(|w| w[0] < w[1]), "in row order");
        assert!(at.iter().all(|i| wide[*i].is_change()), "and every one of them is a change");
    }

    /// Marks join back into the line they came from, which is what lets a window style a row
    /// without rewriting a single character of it.
    #[test]
    fn the_marks_of_a_line_join_back_into_that_line() {
        for line in ["", " ", "one two three", "  spaced   out ", "tab\tseparated"] {
            let (_, marks) = words("nothing here at all", line);
            assert_eq!(marks.iter().map(|m| m.text.as_str()).collect::<String>(), line, "the marks of {line:?} have to read back as it stands");
        }
    }

    /// The client can be asked about a file that no longer exists or does not yet: the diff says
    /// so by naming `/dev/null` on one side, and the window shows one side with no name.
    #[test]
    fn a_file_that_only_exists_on_one_side_has_no_name_on_the_other() {
        let made = changes("diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n");
        let file = made.iter().find(|r| r.kind == Kind::File).expect("the new file");
        assert_eq!(file.text(Side::Old), "", "there is no old file to name");
        assert_eq!(file.text(Side::New), "new.txt");
        assert_eq!(made.iter().filter(|r| r.kind == Kind::Added).count(), 2);
    }

    /// A hunk header the window cannot read costs the line numbers, not the lines: the client's
    /// own answer still has to be shown.
    #[test]
    fn lines_are_still_shown_when_a_hunk_header_says_nothing_useful() {
        let odd = changes("--- a/x\n+++ b/x\nold line\n-only old\n+only new\n");
        assert_eq!(odd.iter().filter(|r| r.kind == Kind::Context).count(), 1, "{odd:?}");
        let pair = odd.iter().find(|r| r.kind == Kind::Changed).expect("the removal and the addition beside it are one edited line: {odd:?}");
        assert_eq!(pair.text(Side::Old), "only old");
        assert_eq!(pair.text(Side::New), "only new");
        assert_eq!(odd.iter().filter(|r| r.kind == Kind::File).count(), 1, "and the file it speaks of is still named: {odd:?}");
    }

    /// A pair of words that traded places is what the reader has to see: the words that kept their
    /// order stand unmarked on both sides, and each side marks its own part of the move.
    #[test]
    fn a_pair_of_words_that_traded_places_is_marked_and_the_word_that_stayed_is_not() {
        let (before, after) = words("alpha beta gamma", "alpha gamma beta");
        let kept = |m: &[Mark]| m.iter().filter(|x| !x.changed).map(|x| x.text.as_str()).collect::<String>();
        let moved = |m: &[Mark]| m.iter().filter(|x| x.changed).map(|x| x.text.as_str()).collect::<String>();
        assert_eq!(kept(&before), kept(&after), "the words both sides keep stand unmarked on each of them: {before:?} against {after:?}");
        assert_eq!(kept(&before), "alpha beta", "the pair that kept its order: {before:?}");
        assert_eq!(moved(&before), " gamma", "and the one that moved off it: {before:?}");
        assert_eq!(moved(&after), " gamma", "on the new side too: {after:?}");
    }

    /// A minified line of thousands of words is one change to read, not a table of its words: past
    /// the point where lining the words up costs more than it tells, the pair is marked whole.
    #[test]
    fn a_pair_of_over_long_lines_is_marked_whole_rather_than_word_by_word() {
        let long = "word ".repeat(INLINE_TOKENS + 10);
        let (before, after) = words(&long, &format!("other {long}"));
        assert_eq!(before.len(), 1, "one run, not a table of {INLINE_TOKENS} cells: {before:?}");
        assert!(before[0].changed);
        assert_eq!(after.iter().map(|m| m.text.as_str()).collect::<String>(), format!("other {long}"), "and the row still reads as the line it came from");
    }

    /// CVS's diff names its file in its own preamble before the shared shape starts.
    #[test]
    fn a_client_preamble_in_its_own_words_still_names_the_file() {
        let cvs = "RCS file: /cvs/module/notes.txt,v\nretrieving revision 1.1\nretrieving revision 1.2\ndiff -u -r1.1 -r1.2\n--- notes.txt\t2024-01-01 09:00:00 -0000\n+++ notes.txt\t2024-01-02 10:00:00 -0000\n@@ -1,2 +1,2 @@\n one\n-old two\n+new two\n";
        let rows = changes(cvs);
        let file = rows.iter().find(|r| r.kind == Kind::File).expect("the file the client reported: {rows:?}");
        assert_eq!(file.text(Side::Old), "notes.txt");
        assert_eq!(file.text(Side::New), "notes.txt");
        assert_eq!(rows.iter().filter(|r| r.kind == Kind::Changed).count(), 1);
    }
}
