//! What Backup Mode and Volume Mountpoints do as the administrator (README: Links): the window
//! starts `failbrauwser --links <command> …` through pkexec, as Link Shell Extension starts its
//! elevated `LSEUacHelper.exe`. The helper names each entry it reaches on its standard output
//! (`at\t<path>`), stops when its standard input says `stop` (or when nobody reads its output
//! any more), and ends with `report\t<JSON>`.
//!
//! Commands (Backup Mode always on):
//!
//! ```text
//! tree <smart-copy|smart-mirror|hardlink-clone|symlink-clone> <outer> <relative 0|1> <src> <dst> [<src> <dst>…]
//! delorean <outer> <relative 0|1> <stamp> <src> <dest folder>
//! smart-move <relative 0|1> <old> <new> [<old> <new>…]
//! retarget <relative 0|1> <link> <target>
//! mount <top folder of a drive> <empty folder>
//! unmount <mountpoint folder>
//! ```

use super::tree::{self, Outer, TreeOp};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The option that starts the helper.
pub const ARG: &str = "--links";

/// What a helper run did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub copied: usize,
    pub linked: usize,
    pub kept: usize,
    pub removed: usize,
    pub folders: usize,
    /// (path, what went wrong)
    pub errors: Vec<(String, String)>,
    pub cancelled: bool,
}

impl Reply {
    fn of(r: &tree::Report) -> Reply {
        Reply {
            copied: r.copied,
            linked: r.linked,
            kept: r.kept,
            removed: r.removed,
            folders: r.folders,
            errors: r.errors.iter().map(|(p, e)| (p.display().to_string(), e.clone())).collect(),
            cancelled: r.cancelled,
        }
    }

    fn failed(p: &Path, e: &io::Error) -> Reply {
        Reply { errors: vec![(p.display().to_string(), e.to_string())], ..Reply::default() }
    }

    /// The run as a tree report (paths as the helper wrote them).
    pub fn report(&self) -> tree::Report {
        tree::Report {
            copied: self.copied,
            linked: self.linked,
            kept: self.kept,
            removed: self.removed,
            folders: self.folders,
            errors: self.errors.iter().map(|(p, e)| (PathBuf::from(p), e.clone())).collect(),
            cancelled: self.cancelled,
        }
    }

    /// The first error as an error, for single acts (mount, unmount, retarget).
    pub fn result(&self) -> io::Result<()> {
        match self.errors.first() {
            Some((_, e)) => Err(io::Error::other(e.clone())),
            None => Ok(()),
        }
    }
}

/// How the helper is started.
#[derive(Debug, Clone)]
pub struct Elevation {
    argv: Vec<OsString>,
}

impl Elevation {
    /// `exe` (failBrauwser itself) as the administrator: through pkexec, directly when this
    /// already is the administrator. `FAILBRAUWSER_ELEVATE` names another program to start it
    /// through ("unshare -r" in the tests: root of a user namespace, no password).
    pub fn admin(exe: &Path) -> Elevation {
        if let Some(prefix) = std::env::var_os("FAILBRAUWSER_ELEVATE").filter(|p| !p.is_empty()) {
            let mut argv: Vec<OsString> = prefix.to_string_lossy().split_whitespace().map(OsString::from).collect();
            argv.push(exe.into());
            return Elevation { argv };
        }
        #[cfg(unix)]
        // SAFETY: no arguments, no memory.
        if unsafe { libc::geteuid() } == 0 {
            return Elevation { argv: vec![exe.into()] };
        }
        Elevation { argv: vec!["pkexec".into(), exe.into()] }
    }

    /// The helper started by `argv` (a program and its first arguments), `--links …` appended.
    pub fn with(argv: Vec<OsString>) -> Elevation {
        Elevation { argv }
    }

    /// Runs the helper command `args`; `progress` is told each entry it reaches and stops it by
    /// answering false.
    pub fn run(&self, args: Vec<OsString>, progress: &mut dyn FnMut(&str) -> bool) -> io::Result<Reply> {
        use std::process::{Command, Stdio};
        let (program, first) = self.argv.split_first().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no helper"))?;
        let mut command = Command::new(program);
        command
            .args(first)
            .arg(ARG)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // tests\source_rules.rs asks this of every start: no window of its own. The elevation run is the
        // Unix path (a Windows build keeps its backup privileges in-process, ui\links.rs), but a Windows
        // build can still reach it through FAILBRAUWSER_ELEVATE. pkexec's own auth dialog is the point.
        #[cfg(windows)]
        crate::win::console::no_window(&mut command);
        let mut child = command
            .spawn()
            .map_err(|e| if e.kind() == io::ErrorKind::NotFound { io::Error::new(e.kind(), format!("cannot start {}: {e}", program.to_string_lossy())) } else { e })?;
        let mut stdin = child.stdin.take();
        let stderr = child.stderr.take();
        // Read meanwhile, so a talkative helper never blocks on a full pipe.
        let errs = std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut e) = stderr {
                let _ = io::Read::read_to_string(&mut e, &mut s);
            }
            s
        });
        let mut reply = None;
        if let Some(out) = child.stdout.take() {
            for line in io::BufReader::new(out).split(b'\n') {
                let line = String::from_utf8_lossy(&line?).into_owned();
                if let Some(p) = line.strip_prefix("at\t") {
                    if !progress(p) {
                        if let Some(mut i) = stdin.take() {
                            let _ = i.write_all(b"stop\n");
                        }
                    }
                } else if let Some(json) = line.strip_prefix("report\t") {
                    reply = serde_json::from_str::<Reply>(json).ok();
                }
            }
        }
        drop(stdin);
        let status = child.wait()?;
        let errs = errs.join().unwrap_or_default();
        match reply {
            Some(r) => Ok(r),
            // pkexec: 126 when the password dialog was dismissed, 127 when it was refused.
            None if self.argv.first().is_some_and(|p| p == "pkexec") && matches!(status.code(), Some(126 | 127)) => Err(io::Error::new(io::ErrorKind::PermissionDenied, "the administrator password was not given")),
            None => Err(io::Error::other(if errs.trim().is_empty() { format!("the helper ended without an answer ({status})") } else { errs.trim().to_string() })),
        }
    }
}

/// `failbrauwser --links …` (the arguments after `--links`); gives the exit code.
pub fn main(args: &[OsString]) -> i32 {
    #[cfg(unix)]
    // SAFETY: no arguments, no memory.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("failbrauwser --links must run as the administrator (through pkexec)");
        return 1;
    }
    let _ = super::backup::enable_privileges();
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        std::thread::spawn(move || {
            // A closed input is no stop: started without one, it reads as closed at once.
            let mut line = String::new();
            loop {
                line.clear();
                match io::stdin().lock().read_line(&mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) if line.trim() == "stop" => break,
                    Ok(_) => {}
                }
            }
            stop.store(true, Ordering::Relaxed);
        });
    }
    let mut progress = |p: &Path| {
        let shown = p.display().to_string().replace(['\n', '\r'], " ");
        let ok = writeln!(io::stdout(), "at\t{shown}").is_ok();
        ok && !stop.load(Ordering::Relaxed)
    };
    let reply = match command(args, &mut progress) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failbrauwser --links: {e}");
            return 2;
        }
    };
    let json = serde_json::to_string(&reply).unwrap_or_default();
    if writeln!(io::stdout(), "report\t{json}").is_err() {
        return 1;
    }
    0
}

fn command(args: &[OsString], progress: &mut dyn FnMut(&Path) -> bool) -> Result<Reply, String> {
    let s = |i: usize| args.get(i).and_then(|a| a.to_str()).ok_or_else(|| format!("argument {i} missing"));
    let p = |i: usize| args.get(i).map(PathBuf::from).ok_or_else(|| format!("argument {i} missing"));
    let flag = |i: usize| s(i).map(|v| v == "1");
    let pairs = |from: usize| -> Result<Vec<(PathBuf, PathBuf)>, String> {
        let rest = args.get(from..).unwrap_or_default();
        if rest.is_empty() || rest.len() % 2 != 0 {
            return Err("paths come in pairs".into());
        }
        Ok(rest.chunks(2).map(|c| (PathBuf::from(&c[0]), PathBuf::from(&c[1]))).collect())
    };
    match s(0)? {
        "tree" => {
            let op = TreeOp::from_id(s(1)?).ok_or("unknown tree operation")?;
            let opts = tree::Options { outer: Outer::from_id(s(2)?), relative: flag(3)?, backup: true };
            Ok(Reply::of(&tree::run(op, &pairs(4)?, opts, progress)))
        }
        "delorean" => {
            let opts = tree::Options { outer: Outer::from_id(s(1)?), relative: flag(2)?, backup: true };
            let (src, dir) = (p(4)?, p(5)?);
            Ok(match tree::delorean(&src, &dir, s(3)?, opts, progress) {
                Ok((_, r)) => Reply::of(&r),
                Err(e) => Reply::failed(&dir, &e),
            })
        }
        "smart-move" => {
            let relative = flag(1)?;
            let mut r = Reply::default();
            for (old, new) in pairs(2)? {
                progress(&new);
                let fixed = super::moved::after_move(&old, &new, relative, true);
                r.linked += fixed.links.len();
                r.errors.extend(fixed.errors.iter().map(|(p, e)| (p.display().to_string(), e.clone())));
            }
            Ok(r)
        }
        "retarget" => {
            let (link, target) = (p(2)?, p(3)?);
            Ok(match super::make::retarget_with(&link, &target, flag(1)?, true) {
                Ok(()) => Reply { linked: 1, ..Reply::default() },
                Err(e) => Reply::failed(&link, &e),
            })
        }
        #[cfg(target_os = "linux")]
        "mount" => {
            let (src, folder) = (p(1)?, p(2)?);
            let owner = std::env::var("PKEXEC_UID").ok().and_then(|u| u.parse().ok());
            Ok(match super::volume::mount_here(&src, &folder, owner) {
                Ok(()) => Reply { linked: 1, ..Reply::default() },
                Err(e) => Reply::failed(&folder, &e),
            })
        }
        #[cfg(target_os = "linux")]
        "unmount" => {
            let folder = p(1)?;
            Ok(match super::volume::unmount_here(&folder) {
                Ok(()) => Reply { removed: 1, ..Reply::default() },
                Err(e) => Reply::failed(&folder, &e),
            })
        }
        other => Err(format!("unknown command {other:?}")),
    }
}

/// The helper arguments of a tree run.
pub fn tree_args(op: TreeOp, opts: tree::Options, pairs: &[(PathBuf, PathBuf)]) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["tree".into(), op.id().into(), opts.outer.id().into(), bit(opts.relative)];
    for (s, d) in pairs {
        a.push(s.into());
        a.push(d.into());
    }
    a
}

/// The helper arguments of a DeLorean Copy.
pub fn delorean_args(opts: tree::Options, stamp: &str, source: &Path, dest_dir: &Path) -> Vec<OsString> {
    vec!["delorean".into(), opts.outer.id().into(), bit(opts.relative), stamp.into(), source.into(), dest_dir.into()]
}

/// The helper arguments of Smart Move after `pairs` (old, new) moved.
pub fn smart_move_args(relative: bool, pairs: &[(PathBuf, PathBuf)]) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["smart-move".into(), bit(relative)];
    for (o, n) in pairs {
        a.push(o.into());
        a.push(n.into());
    }
    a
}

/// The helper arguments of *Replace Symbolic Link / Junction*.
pub fn retarget_args(relative: bool, link: &Path, target: &Path) -> Vec<OsString> {
    vec!["retarget".into(), bit(relative), link.into(), target.into()]
}

fn bit(b: bool) -> OsString {
    if b { "1" } else { "0" }.into()
}
