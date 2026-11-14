//! What the windows ask a working copy to do, as the argument lists of its tool's own client
//! (the README's capability table, the command column). Every list runs inside the working copy,
//! paths relative to its top ("" for the whole of it); a tool without the item answers no lists,
//! and so does an item missing the words it needs.

use super::caps::Cap;
use super::Tool;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Commits exactly `paths`; unversioned ones among them are added first by the window.
    Commit { paths: Vec<String>, message: String, amend: bool },
    Add { paths: Vec<String> },
    Revert { paths: Vec<String> },
    Delete { paths: Vec<String>, keep_local: bool },
    Rename { from: String, to: String },
    Resolve { paths: Vec<String> },
    /// A central tool's update; Git's and Mercurial's pull.
    Update,
    Pull,
    Fetch,
    Push,
    Switch { to: String },
    /// The working copy at an earlier revision of the history (the log's right-click menu).
    UpdateTo { rev: String },
    Merge { from: String },
    Branch { name: String, message: String },
    Tag { name: String, message: String },
    Export { dest: String },
    Cleanup,
    Lock { paths: Vec<String>, message: String },
    Unlock { paths: Vec<String> },
    StashSave { message: String },
    StashPop,
    PatchCreate { dest: String },
    PatchApply { file: String },
    Clone { url: String, dir: String },
    Init { dir: String },
}

impl Op {
    /// The capability an item needs.
    pub fn cap(&self) -> Cap {
        match self {
            Op::Commit { amend: true, .. } => Cap::Amend,
            Op::Commit { .. } => Cap::Commit,
            Op::Add { .. } => Cap::Add,
            Op::Revert { .. } => Cap::Revert,
            Op::Delete { keep_local: true, .. } => Cap::DeleteKeepLocal,
            Op::Delete { .. } => Cap::Delete,
            Op::Rename { .. } => Cap::Rename,
            Op::Resolve { .. } => Cap::Resolve,
            Op::Update => Cap::Update,
            Op::Pull => Cap::Pull,
            Op::Fetch => Cap::Fetch,
            Op::Push => Cap::Push,
            Op::Switch { .. } | Op::UpdateTo { .. } => Cap::Switch,
            Op::Merge { .. } => Cap::Merge,
            Op::Branch { .. } => Cap::Branch,
            Op::Tag { .. } => Cap::Tag,
            Op::Export { .. } => Cap::Export,
            Op::Cleanup => Cap::Cleanup,
            Op::Lock { .. } | Op::Unlock { .. } => Cap::Lock,
            Op::StashSave { .. } | Op::StashPop => Cap::Stash,
            Op::PatchCreate { .. } => Cap::PatchCreate,
            Op::PatchApply { .. } => Cap::PatchApply,
            Op::Clone { .. } => Cap::Clone,
            Op::Init { .. } => Cap::Init,
        }
    }
}

/// Paths as the client takes them: the whole working copy is ".".
fn targets(paths: &[String]) -> Vec<String> {
    if paths.is_empty() || paths.iter().any(String::is_empty) { vec![".".into()] } else { paths.to_vec() }
}

fn list(words: &[&str], paths: &[String]) -> Vec<String> {
    let mut v: Vec<String> = words.iter().map(|s| s.to_string()).collect();
    v.push("--".into());
    v.extend(targets(paths));
    v
}

fn plain(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

/// Visual Source Safe takes no `--`; its options sit before the names, a comment glued to `-C`.
fn ss(words: &[&str], paths: &[String]) -> Vec<String> {
    let mut v = plain(words);
    v.extend(targets(paths));
    v
}

/// The client calls `op` means for `tool`, in order.
pub fn runs(tool: Tool, op: &Op) -> Vec<Vec<String>> {
    use Tool::*;
    if !tool.has(op.cap()) {
        return Vec::new();
    }
    let one = |v: Vec<String>| vec![v];
    match op {
        Op::Commit { message, .. } if message.trim().is_empty() => Vec::new(),
        Op::Commit { paths, message, amend } => match tool {
            // `--only` commits exactly what is named, whatever else the index holds.
            Git => {
                let mut v = plain(&["commit", "--only", "-m", message]);
                if *amend {
                    v.insert(1, "--amend".into());
                }
                v.push("--".into());
                v.extend(targets(paths));
                one(v)
            }
            Mercurial if *amend => one(list(&["commit", "--amend", "-m", message], paths)),
            Mercurial | Subversion => one(list(&["commit", "-m", message], paths)),
            // CVS 1.11 takes no `--` after its options.
            Cvs => one(ss(&["commit", "-m", message], paths)),
            VisualSourceSafe => one(ss(&["Checkin", "-I-Y", &format!("-C{message}")], paths)),
        },
        Op::Add { paths } => match tool {
            Git | Mercurial => one(list(&["add"], paths)),
            Subversion => one(list(&["add", "--parents"], paths)),
            Cvs => one(ss(&["add"], paths)),
            VisualSourceSafe => one(ss(&["Add", "-I-Y"], paths)),
        },
        Op::Revert { paths } => match tool {
            Git => one(list(&["restore", "--staged", "--worktree", "--source=HEAD"], paths)),
            Mercurial => one(list(&["revert", "--no-backup"], paths)),
            Subversion => one(list(&["revert", "--recursive"], paths)),
            Cvs => one(ss(&["update", "-C"], paths)),
            VisualSourceSafe => one(ss(&["UndoCheckout", "-I-Y"], paths)),
        },
        Op::Delete { paths, keep_local } => match (tool, keep_local) {
            (Git, false) => one(list(&["rm", "-r"], paths)),
            (Git, true) => one(list(&["rm", "-r", "--cached"], paths)),
            (Mercurial, false) => one(list(&["remove"], paths)),
            (Mercurial, true) => one(list(&["forget"], paths)),
            (Subversion, false) => one(list(&["delete"], paths)),
            (Subversion, true) => one(list(&["delete", "--keep-local"], paths)),
            // `-f` takes the file away too; the window puts it back for "keep local".
            (Cvs, false) => one(ss(&["remove", "-f"], paths)),
            (Cvs, true) => one(ss(&["remove"], paths)),
            (VisualSourceSafe, _) => one(ss(&["Delete", "-I-Y"], paths)),
        },
        Op::Rename { from, to } => match tool {
            Git => one(plain(&["mv", "--", from, to])),
            Mercurial => one(plain(&["rename", "--", from, to])),
            Subversion => one(plain(&["move", "--", from, to])),
            VisualSourceSafe => one(plain(&["Rename", "-I-Y", from, to])),
            Cvs => Vec::new(),
        },
        Op::Resolve { paths } => match tool {
            Git => one(list(&["add"], paths)),
            Mercurial => one(list(&["resolve", "--mark"], paths)),
            Subversion => one(list(&["resolve", "--accept", "working"], paths)),
            _ => Vec::new(),
        },
        Op::Update => match tool {
            Subversion => one(plain(&["update"])),
            Cvs => one(plain(&["update", "-d"])),
            VisualSourceSafe => one(plain(&["Get", "-R", "-I-Y"])),
            _ => Vec::new(),
        },
        Op::Pull => match tool {
            Git => one(plain(&["pull"])),
            Mercurial => one(plain(&["pull", "--update"])),
            _ => Vec::new(),
        },
        Op::Fetch => match tool {
            Git => one(plain(&["fetch"])),
            Mercurial => one(plain(&["pull"])),
            _ => Vec::new(),
        },
        Op::Push => one(plain(&["push"])),
        Op::Switch { to } if to.trim().is_empty() => Vec::new(),
        Op::Switch { to } => match tool {
            Git => one(plain(&["switch", to])),
            Mercurial => one(plain(&["update", "--", to])),
            Subversion => one(plain(&["switch", "--", to])),
            Cvs => one(plain(&["update", "-r", to])),
            VisualSourceSafe => one(plain(&["Get", "-R", "-I-Y", &format!("-V{to}")])),
        },
        Op::UpdateTo { rev } if rev.trim().is_empty() => Vec::new(),
        Op::UpdateTo { rev } => match tool {
            Git => one(plain(&["checkout", rev])),
            Mercurial => one(plain(&["update", "--rev", rev])),
            Subversion => one(plain(&["update", "--revision", rev])),
            // A CVS changeset is named by its time; a file revision has a dot.
            Cvs if rev.contains('.') => one(plain(&["update", "-r", rev])),
            Cvs => one(plain(&["update", "-D", &super::cvs::date_arg(rev)])),
            VisualSourceSafe => one(plain(&["Get", "-R", "-I-Y", &format!("-V{rev}")])),
        },
        Op::Merge { from } if from.trim().is_empty() => Vec::new(),
        Op::Merge { from } => match tool {
            Git => one(plain(&["merge", "--no-edit", from])),
            Mercurial => one(plain(&["merge", "--", from])),
            Subversion => one(plain(&["merge", "--", from])),
            Cvs => one(plain(&["update", "-j", from])),
            VisualSourceSafe => Vec::new(),
        },
        Op::Branch { name, .. } | Op::Tag { name, .. } if name.trim().is_empty() => Vec::new(),
        Op::Branch { name, message } => match tool {
            Git => one(plain(&["branch", "--", name])),
            Mercurial => one(plain(&["branch", "--", name])),
            Subversion => one(plain(&["copy", "-m", message, "--", "^/trunk", &format!("^/branches/{name}")])),
            Cvs => one(plain(&["tag", "-b", name])),
            VisualSourceSafe => Vec::new(),
        },
        Op::Tag { name, message } => match tool {
            Git => one(plain(&["tag", "-m", message, "--", name])),
            Mercurial => one(plain(&["tag", "-m", message, "--", name])),
            Subversion => one(plain(&["copy", "-m", message, "--", ".", &format!("^/tags/{name}")])),
            Cvs => one(plain(&["tag", name])),
            VisualSourceSafe => one(plain(&["Label", "-I-Y", &format!("-L{name}"), &format!("-C{message}")])),
        },
        Op::Export { dest } => match tool {
            Git => one(plain(&["archive", "--format=zip", "-o", dest, "HEAD"])),
            Mercurial => one(plain(&["archive", "--", dest])),
            Subversion => one(plain(&["export", "--", ".", dest])),
            // CVS exports a module by name from its repository, by the date or tag the window gives.
            Cvs => one(plain(&["export", "-r", "HEAD", "-d", dest, "."])),
            VisualSourceSafe => Vec::new(),
        },
        Op::Cleanup => match tool {
            Git => one(plain(&["clean", "-d", "-f"])),
            Mercurial => one(plain(&["--config", "extensions.purge=", "purge"])),
            Subversion => one(plain(&["cleanup", "--remove-unversioned"])),
            _ => Vec::new(),
        },
        Op::Lock { paths, message } => match tool {
            Subversion => one(list(&["lock", "-m", message], paths)),
            VisualSourceSafe => one(ss(&["Checkout", "-I-Y", &format!("-C{message}")], paths)),
            _ => Vec::new(),
        },
        Op::Unlock { paths } => match tool {
            Subversion => one(list(&["unlock"], paths)),
            VisualSourceSafe => one(ss(&["UndoCheckout", "-I-Y"], paths)),
            _ => Vec::new(),
        },
        Op::StashSave { message } => match tool {
            Git if message.is_empty() => one(plain(&["stash", "push"])),
            Git => one(plain(&["stash", "push", "-m", message])),
            Mercurial if message.is_empty() => one(plain(&["shelve"])),
            Mercurial => one(plain(&["shelve", "-m", message])),
            _ => Vec::new(),
        },
        Op::StashPop => match tool {
            Git => one(plain(&["stash", "pop"])),
            Mercurial => one(plain(&["unshelve"])),
            _ => Vec::new(),
        },
        Op::PatchCreate { dest } => match tool {
            Git => one(plain(&["format-patch", "-1", "-o", dest])),
            Mercurial => one(plain(&["export", "--output", dest, "--rev", "."])),
            // Subversion and CVS write their diff to standard output; the window saves it as `dest`.
            Subversion => one(plain(&["diff"])),
            Cvs => one(plain(&["diff", "-u"])),
            VisualSourceSafe => Vec::new(),
        },
        Op::PatchApply { file } => match tool {
            Git => one(plain(&["am", "--", file])),
            Mercurial => one(plain(&["import", "--", file])),
            Subversion => one(plain(&["patch", "--", file])),
            _ => Vec::new(),
        },
        Op::Clone { url, dir } => match tool {
            Git | Mercurial => one(plain(&["clone", "--", url, dir])),
            Subversion => one(plain(&["checkout", "--", url, dir])),
            // CVS's address is "<CVSROOT> <module>": the root goes before the command.
            Cvs => match url.rsplit_once(' ') {
                Some((root, module)) => one(plain(&["-d", root.trim(), "checkout", "-d", dir, module.trim()])),
                None => Vec::new(),
            },
            VisualSourceSafe => Vec::new(),
        },
        Op::Init { dir } => one(plain(&["init", "--", dir])),
    }
}

/// The ignore file a name is appended to, where the tool keeps one (Subversion keeps a property).
pub fn ignore_file(tool: Tool) -> Option<&'static str> {
    match tool {
        Tool::Git => Some(".gitignore"),
        Tool::Mercurial => Some(".hgignore"),
        Tool::Cvs => Some(".cvsignore"),
        Tool::Subversion | Tool::VisualSourceSafe => None,
    }
}

/// Subversion's ignore list of a folder with `pattern` added: the property's lines, kept once.
pub fn svn_ignore_value(current: &str, pattern: &str) -> String {
    let mut lines: Vec<&str> = current.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if !lines.contains(&pattern) {
        lines.push(pattern);
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn commit(paths: &[&str]) -> Op {
        Op::Commit { paths: s(paths), message: "msg".into(), amend: false }
    }

    #[test]
    fn a_commit_runs_each_tool_s_own_commit_with_the_message_and_only_the_ticked_files() {
        assert_eq!(runs(Tool::Git, &commit(&["a.txt"])), [s(&["commit", "--only", "-m", "msg", "--", "a.txt"])]);
        assert_eq!(runs(Tool::Mercurial, &commit(&["a.txt"])), [s(&["commit", "-m", "msg", "--", "a.txt"])]);
        assert_eq!(runs(Tool::Subversion, &commit(&["a.txt"])), [s(&["commit", "-m", "msg", "--", "a.txt"])]);
        assert_eq!(runs(Tool::Cvs, &commit(&["a.txt"])), [s(&["commit", "-m", "msg", "a.txt"])]);
        assert_eq!(runs(Tool::VisualSourceSafe, &commit(&["a.txt"])), [s(&["Checkin", "-I-Y", "-Cmsg", "a.txt"])]);
    }

    #[test]
    fn the_whole_working_copy_is_named_as_its_top() {
        assert_eq!(runs(Tool::Subversion, &Op::Revert { paths: vec![String::new()] }), [s(&["revert", "--recursive", "--", "."])]);
    }

    #[test]
    fn a_commit_without_a_message_runs_nothing() {
        assert!(runs(Tool::Git, &Op::Commit { paths: vec![], message: "  ".into(), amend: false }).is_empty());
    }

    #[test]
    fn what_a_tool_lacks_runs_nothing() {
        assert!(runs(Tool::Cvs, &Op::Rename { from: "a".into(), to: "b".into() }).is_empty());
        assert!(runs(Tool::Subversion, &Op::Push).is_empty());
        assert!(runs(Tool::Subversion, &Op::Commit { paths: vec![], message: "m".into(), amend: true }).is_empty(), "amending needs a local commit");
        assert!(runs(Tool::VisualSourceSafe, &Op::Merge { from: "x".into() }).is_empty());
        for tool in Tool::all() {
            for op in [Op::Update, Op::Pull, Op::Push, Op::Cleanup, Op::StashPop] {
                assert_eq!(runs(*tool, &op).is_empty(), !tool.has(op.cap()), "{op:?} on {}", tool.label());
            }
        }
    }

    #[test]
    fn subversion_branches_and_tags_are_copies_in_the_repository() {
        assert_eq!(runs(Tool::Subversion, &Op::Branch { name: "fix".into(), message: "m".into() }), [s(&["copy", "-m", "m", "--", "^/trunk", "^/branches/fix"])]);
        assert_eq!(runs(Tool::Subversion, &Op::Tag { name: "v1".into(), message: "m".into() }), [s(&["copy", "-m", "m", "--", ".", "^/tags/v1"])]);
        assert_eq!(runs(Tool::Cvs, &Op::Branch { name: "fix".into(), message: "m".into() }), [s(&["tag", "-b", "fix"])]);
    }

    #[test]
    fn a_cvs_address_is_its_root_and_its_module() {
        assert_eq!(runs(Tool::Cvs, &Op::Clone { url: ":pserver:me@host:/cvs mod".into(), dir: "out".into() }), [s(&["-d", ":pserver:me@host:/cvs", "checkout", "-d", "out", "mod"])]);
        assert!(runs(Tool::Cvs, &Op::Clone { url: "nomodule".into(), dir: "out".into() }).is_empty());
    }

    #[test]
    fn updating_to_a_revision_of_the_history() {
        assert_eq!(runs(Tool::Subversion, &Op::UpdateTo { rev: "7".into() }), [s(&["update", "--revision", "7"])]);
        assert_eq!(runs(Tool::Cvs, &Op::UpdateTo { rev: "1.3".into() }), [s(&["update", "-r", "1.3"])]);
        assert_eq!(runs(Tool::Cvs, &Op::UpdateTo { rev: "1794623599".into() }), [s(&["update", "-D", "2026-11-14 02:33:19 UTC"])]);
    }

    #[test]
    fn delete_keep_local_leaves_the_file() {
        assert_eq!(runs(Tool::Git, &Op::Delete { paths: s(&["a"]), keep_local: true }), [s(&["rm", "-r", "--cached", "--", "a"])]);
        assert_eq!(runs(Tool::Mercurial, &Op::Delete { paths: s(&["a"]), keep_local: true }), [s(&["forget", "--", "a"])]);
        assert_eq!(runs(Tool::Subversion, &Op::Delete { paths: s(&["a"]), keep_local: true }), [s(&["delete", "--keep-local", "--", "a"])]);
    }

    #[test]
    fn an_ignored_name_is_kept_once_in_the_property() {
        assert_eq!(svn_ignore_value("a\n*.o\n", "*.o"), "a\n*.o\n");
        assert_eq!(svn_ignore_value("", "build"), "build\n");
    }
}
