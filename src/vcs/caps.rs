//! What each tool can be asked to do: the capability table of the README's *Version control*
//! section, item for item. The menu shows an item only where the folder's tool has it, and the
//! windows hide the controls a tool has no equivalent for, so this one table decides both. A test
//! reads the table out of the README and compares, so the specification and the program cannot
//! tell two different stories.

use super::Tool;

/// An item of the right-click menu or a control of a window that not every tool has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cap {
    /// *⟨Tool⟩ Sync…* at the top level: fetch, pull and push in one window.
    Sync,
    /// *⟨Tool⟩ Update* at the top level: a central tool fetches and merges in one step.
    Update,
    Commit,
    Pull,
    Fetch,
    Push,
    Diff,
    DiffPrevious,
    Log,
    Reflog,
    Blame,
    CheckModifications,
    Stash,
    Resolve,
    Revert,
    Cleanup,
    Lock,
    Switch,
    Merge,
    Branch,
    Tag,
    Export,
    Add,
    Delete,
    DeleteKeepLocal,
    Rename,
    Ignore,
    PatchCreate,
    PatchApply,
    Clone,
    Init,
    Settings,
    /// *Amend Last Commit* in the Commit window.
    Amend,
    /// The revision graph of *Log Messages*: only where the tool records parents.
    Graph,
}

impl Cap {
    /// Every capability, in the order the README table lists the items (the window-only ones last).
    pub fn all() -> &'static [Cap] {
        use Cap::*;
        &[
            Sync, Update, Commit, Pull, Fetch, Push, Diff, DiffPrevious, Log, Reflog, Blame, CheckModifications, Stash, Resolve, Revert, Cleanup, Lock, Switch, Merge, Branch, Tag, Export, Add, Delete,
            DeleteKeepLocal, Rename, Ignore, PatchCreate, PatchApply, Clone, Init, Settings, Amend, Graph,
        ]
    }

    /// The row of the README table this capability is, where it has one.
    pub fn readme_row(self) -> Option<&'static str> {
        use Cap::*;
        Some(match self {
            Sync => "Sync… (top level)",
            Update => "Update (top level)",
            Commit => "Commit -> \"branch\"… (top level)",
            Pull => "Pull…",
            Fetch => "Fetch…",
            Push => "Push…",
            Diff => "Diff",
            DiffPrevious => "Diff with previous version",
            Log => "Show log",
            Reflog => "Show Reflog",
            Blame => "Blame",
            CheckModifications => "Check for modifications",
            Stash => "Stash Save / Stash Pop",
            Resolve => "Resolve…",
            Revert => "Revert…",
            Cleanup => "Clean up…",
            Lock => "Get Lock… / Release Lock",
            Switch => "Switch/Checkout…",
            Merge => "Merge…",
            Branch => "Create Branch…",
            Tag => "Create Tag…",
            Export => "Export…",
            Add => "Add…",
            Delete => "Delete",
            DeleteKeepLocal => "Delete (keep local)",
            Rename => "Rename…",
            Ignore => "Add to ignore list",
            PatchCreate => "Create Patch Serial…",
            PatchApply => "Apply Patch Serial…",
            Clone => "Clone… (outside)",
            Init => "Create repository here… (outside)",
            Settings => "Settings, About",
            Amend | Graph => return None,
        })
    }
}

impl Tool {
    /// Whether this tool has the item or control at all.
    pub fn has(self, cap: Cap) -> bool {
        use Cap::*;
        use Tool::*;
        let distributed = matches!(self, Git | Mercurial);
        match cap {
            Sync | Pull | Fetch | Push | Stash | Init | Amend | Graph => distributed,
            Update => !distributed,
            Reflog => self == Git,
            Blame | Ignore | Export | Branch | Merge => self != VisualSourceSafe,
            Resolve | Cleanup | PatchApply => matches!(self, Git | Mercurial | Subversion),
            Lock => matches!(self, Subversion | VisualSourceSafe),
            DeleteKeepLocal => self != VisualSourceSafe,
            Rename => self != Cvs,
            PatchCreate | Clone => self != VisualSourceSafe,
            Commit | Diff | DiffPrevious | Log | CheckModifications | Revert | Switch | Tag | Add | Delete | Settings => true,
        }
    }

    /// The word the top-level items start with: TortoiseGit says "Git Sync…", TortoiseSVN "SVN
    /// Update", TortoiseHg "Hg", TortoiseCVS "CVS".
    pub fn short(self) -> &'static str {
        match self {
            Tool::Git => "Git",
            Tool::Mercurial => "Hg",
            Tool::Subversion => "SVN",
            Tool::Cvs => "CVS",
            Tool::VisualSourceSafe => "VSS",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The README's capability table: item → one cell per tool, in `Tool::all` order.
    fn readme_table() -> Vec<(String, Vec<String>)> {
        let readme = include_str!("../../README.md");
        let start = readme.find("| Item | Git | Mercurial | Subversion | CVS | Visual Source Safe |").expect("the README has the capability table");
        readme[start..]
            .lines()
            .skip(2)
            .take_while(|l| l.starts_with('|'))
            .map(|l| {
                let cells: Vec<String> = l.trim_matches('|').split('|').map(|c| c.trim().to_string()).collect();
                (cells[0].clone(), cells[1..].to_vec())
            })
            .collect()
    }

    #[test]
    fn the_capabilities_are_the_readme_table() {
        let table = readme_table();
        for cap in Cap::all() {
            let Some(row) = cap.readme_row() else { continue };
            let (_, cells) = table.iter().find(|(item, _)| item == row).unwrap_or_else(|| panic!("the README table has no row {row:?}"));
            for (tool, cell) in Tool::all().iter().zip(cells) {
                assert_eq!(tool.has(*cap), cell != "—", "{row} for {}: the README says {cell:?}", tool.label());
            }
        }
    }

    #[test]
    fn every_row_of_the_readme_table_is_a_capability() {
        let rows: Vec<&str> = Cap::all().iter().filter_map(|c| c.readme_row()).collect();
        for (item, _) in readme_table() {
            assert!(rows.contains(&item.as_str()), "the README lists {item:?}, which no capability answers");
        }
    }

    /// The graph needs parents, and amending needs a local commit to rewrite: both are the
    /// distributed tools' alone.
    #[test]
    fn only_tools_that_record_parents_draw_a_graph() {
        for t in Tool::all() {
            assert_eq!(t.has(Cap::Graph), matches!(t, Tool::Git | Tool::Mercurial), "{}", t.label());
            assert_eq!(t.has(Cap::Amend), t.has(Cap::Graph), "{}", t.label());
        }
    }

    #[test]
    fn a_tool_either_syncs_or_updates() {
        for t in Tool::all() {
            assert_ne!(t.has(Cap::Sync), t.has(Cap::Update), "{}", t.label());
        }
    }
}
