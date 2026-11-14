//! Asking a working copy's own client (M15). One call runs the program `Tool::cli` names, inside
//! the working copy, with the arguments `Command::runs` built; the answer keeps both streams of
//! the client's own words, in the order they arrived. Nothing here touches a repository's own
//! files, so whatever the tool decides about its data is what the window then shows — including
//! the tool's own complaint, which is the sentence a user can act on.

use super::Tool;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// What a client call answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub ok: bool,
    /// The client's words, both streams, newlines as the window wants them.
    pub text: String,
    /// The client's own exit code, where it ran and refused; `None` when it ran fine or could not
    /// start at all.
    pub code: Option<i32>,
}

impl Answer {
    fn refused(code: Option<i32>, text: String) -> Self {
        Self { ok: false, text, code }
    }
}

/// Runs one argument list in `dir` and keeps the answer. A client that is not installed answers
/// in plain words rather than as a panic or an empty success, because four of the five tools this
/// program speaks for are absent on most machines — and on this host they are.
pub fn call(tool: Tool, dir: &Path, argv: &[impl AsRef<std::ffi::OsStr>]) -> Answer {
    ask(tool.cli(), dir, argv)
}

/// The same, for a program named directly: the shape of the answer does not depend on which tool
/// a scenario is out to talk to.
pub fn ask(program: &str, dir: &Path, argv: &[impl AsRef<std::ffi::OsStr>]) -> Answer {
    let mut command = Command::new(program);
    command.current_dir(dir).args(argv).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // The environment the clients read: a locale of its own for the machine-readable output, and
    // no pager standing between us and the answer.
    command.env("LC_ALL", "C").env("LANG", "C").env("PAGER", "").env("GIT_PAGER", "").env("HGPLAIN", "1").env("GIT_TERMINAL_PROMPT", "0").env("SVN_INTERACTIVE", "false").env("SSH_ASKPASS", "").env("GCM_INTERACTIVE", "never");
    // Every client here is a console program: without this each Repository command would bring up
    // a black window, since this program has no console for them to share.
    #[cfg(windows)]
    crate::win::console::no_window(&mut command);
    match command.output() {
        Ok(out) => {
            let text = joined(&out.stdout, &out.stderr);
            let code = out.status.code();
            if out.status.success() {
                return Answer { ok: true, text, code: None };
            }
            // A client that stopped on its own needs no invented sentence: its words are the
            // reason, and the code says how it stopped.
            Answer::refused(code, text)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Answer::refused(None, format!("{program} could not be started: it is not installed, or not on the path.")),
        Err(e) => Answer::refused(None, format!("{program} could not be started: {e}")),
    }
}

/// The calls of one command, in order, stopping at the first that refuses: a command that means
/// several client calls (Sync, a Visual Source Safe revert) has no business running the next one
/// after one of them said no.
pub fn call_in_order(tool: Tool, dir: &Path, runs: &[Vec<String>]) -> Answer {
    let mut answer = Answer { ok: true, text: String::new(), code: None };
    for argv in runs {
        let one = call(tool, dir, argv);
        // Each call of a command starts its own line of the account: a client's last line and the
        // next client's first are unrelated sentences.
        if !answer.text.is_empty() && !one.text.is_empty() {
            answer.text.push('\n');
        }
        answer.text.push_str(&one.text);
        if !one.ok {
            answer.ok = false;
            answer.code = one.code;
            return answer;
        }
    }
    answer
}

/// What a client printed on its standard output, for parsing: machine-readable answers never mix
/// with the client's remarks on the other stream. A refusal answers the client's own words.
pub fn output(program: &str, dir: &Path, argv: &[impl AsRef<std::ffi::OsStr>]) -> Result<Vec<u8>, String> {
    let mut command = Command::new(program);
    command.current_dir(dir).args(argv).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command.env("LC_ALL", "C").env("LANG", "C").env("PAGER", "").env("GIT_PAGER", "").env("HGPLAIN", "1").env("GIT_TERMINAL_PROMPT", "0").env("GIT_OPTIONAL_LOCKS", "0").env("SVN_INTERACTIVE", "false");
    #[cfg(windows)]
    crate::win::console::no_window(&mut command);
    match command.output() {
        Ok(out) if out.status.success() => Ok(out.stdout),
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if err.is_empty() { format!("{program} {} failed", argv.first().map(|a| a.as_ref().to_string_lossy().into_owned()).unwrap_or_default()) } else { err })
        }
        Err(e) => Err(format!("{program} could not be started: {e}")),
    }
}

/// The same, as text.
pub fn text(program: &str, dir: &Path, argv: &[impl AsRef<std::ffi::OsStr>]) -> Result<String, String> {
    output(program, dir, argv).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Both streams of one client, with the empty lines a stream leaves at its end taken out and
/// CRLF made into what the windows read.
fn joined(stdout: &[u8], stderr: &[u8]) -> String {
    let part = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace("\r\n", "\n").trim_matches('\n').to_string();
    let (out, err) = (part(stdout), part(stderr));
    match (out.is_empty(), err.is_empty()) {
        (true, true) => String::new(),
        (true, false) => err,
        (false, true) => out,
        (false, false) => format!("{out}\n{err}"),
    }
}

/// Feeds `input` to a client that reads one (a CVS or Subversion commit asked over a pipe, so no
/// editor or password prompt is ever needed).
pub fn with_input(tool: Tool, dir: &Path, argv: &[impl AsRef<std::ffi::OsStr>], input: &str) -> Answer {
    let mut command = Command::new(tool.cli());
    command.current_dir(dir).args(argv).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).env("LC_ALL", "C").env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    crate::win::console::no_window(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return Answer::refused(None, format!("{} could not be started: {e}", tool.cli())),
    };
    // Writing and closing the input in one scope: a client reading a message ends its read only
    // when the pipe shuts, so an input left open would leave the call waiting forever.
    if let Some(mut stdin) = child.stdin.take() && let Err(e) = stdin.write_all(input.as_bytes()) {
        let _ = child.kill();
        return Answer::refused(None, format!("the words could not reach {}: {e}", tool.cli()));
    }
    match child.wait_with_output() {
        Ok(out) => {
            let text = joined(&out.stdout, &out.stderr);
            if out.status.success() {
                return Answer { ok: true, text, code: None };
            }
            Answer::refused(out.status.code(), text)
        }
        Err(e) => Answer::refused(None, format!("{} did not answer: {e}", tool.cli())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A repository `git` made here, or none where no git client runs.
    fn git_fixture() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
        let probe = Command::new("git").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status();
        if !probe.is_ok_and(|s| s.success()) {
            eprintln!("no git client on this machine, skipping the real-client scenarios");
            return None;
        }
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().to_path_buf();
        let run = |args: &[&str]| {
            let status = Command::new("git").current_dir(&dir).args(args).stdout(Stdio::null()).stderr(Stdio::null()).status().expect("git could not run");
            assert!(status.success(), "git {args:?} did not answer");
        };
        run(&["init", "-q"]);
        Some((d, dir))
    }

    #[test]
    fn a_client_that_is_not_installed_answers_in_words_and_refuses() {
        let answer = ask("no-such-client-9f3c", Path::new("."), &["status"]);
        assert!(!answer.ok);
        assert!(answer.text.contains("no-such-client-9f3c"), "the sentence has to name the client: {}", answer.text);
        assert!(answer.text.contains("not installed") || answer.text.contains("not on the path"), "and say where to look: {}", answer.text);
    }

    #[test]
    fn the_status_of_a_real_repository_arrives_as_the_repository_s_own_words() {
        let Some((_keep, dir)) = git_fixture() else { return };
        let clean = call(super::Tool::Git, &dir, &["status", "--porcelain"]);
        assert!(clean.ok, "a clean repository answers successfully: {}", clean.text);
        assert!(clean.text.is_empty(), "a clean repository has nothing to say: {}", clean.text);

        std::fs::write(dir.join("tracked.txt"), "one\n").unwrap();
        let dirty = call(super::Tool::Git, &dir, &["status", "--porcelain"]);
        assert!(dirty.ok);
        assert!(dirty.text.contains("tracked.txt"), "the file the repository notices has to be in its answer: {}", dirty.text);
    }

    /// A client that refuses is quoted rather than explained away, and its code travels with it:
    /// that is how a switch to a branch that is not there reads in the window.
    #[test]
    fn a_client_that_refuses_keeps_its_own_words_and_its_code() {
        let Some((_keep, dir)) = git_fixture() else { return };
        let answer = call(super::Tool::Git, &dir, &["switch", "no-such-branch-here"]);
        assert!(!answer.ok);
        assert!(!answer.text.is_empty(), "the client's complaint is the whole answer");
        assert!(answer.code.is_some(), "and the code it stopped with travels: {:?}", answer.code);
    }

    /// A command that means several calls stops at the first that refuses, so a push never runs
    /// after a pull the repository said no to.
    #[test]
    fn the_calls_of_one_command_run_in_order_and_stop_at_the_first_refusal() {
        let Some((_keep, dir)) = git_fixture() else { return };
        let runs = vec![vec!["status".to_string(), "--porcelain".to_string()], vec!["switch".to_string(), "absent-branch".to_string()], vec!["status".to_string()]];
        let answer = call_in_order(super::Tool::Git, &dir, &runs);
        assert!(!answer.ok, "a refusal in the middle of a command refuses the command");
        assert!(answer.text.contains("absent-branch"), "the refusal is what is shown: {:?}", answer.text);
        // The third call would have described the empty working tree in its own words; those
        // words being absent is what shows the run stopped at the refusal.
        assert!(!answer.text.contains("nothing to commit"), "the call after the refusal did not run: {:?}", answer.text);
    }

    /// One command can mean several calls (Sync), and the window shows their answers as one
    /// running account: each call's words have to start on a line of their own, or the tail of one
    /// answer runs into the head of the next.
    #[test]
    fn the_answers_of_the_calls_of_one_command_are_kept_apart() {
        let Some((_keep, dir)) = git_fixture() else { return };
        let runs = vec![vec!["status".to_string()], vec!["rev-parse".to_string(), "--git-dir".to_string()]];
        let answer = call_in_order(super::Tool::Git, &dir, &runs);
        assert!(answer.ok, "both calls answered: {:?}", answer.text);
        assert!(answer.text.contains("On branch"), "the first call is quoted: {:?}", answer.text);
        assert!(answer.text.contains(".git"), "and so is the second: {:?}", answer.text);
        assert!(answer.text.contains("\n.git"), "the second answer starts on a line of its own rather than at the end of the first: {:?}", answer.text);
    }

    /// Words a client reads from its input arrive without the client ever asking for an editor.
    #[test]
    fn a_commit_message_reaches_the_client_over_the_pipe() {
        let Some((_keep, dir)) = git_fixture() else { return };
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        let added = call(super::Tool::Git, &dir, &["add", "a.txt"]);
        assert!(added.ok, "{}", added.text);
        let answer = with_input(super::Tool::Git, &dir, &["-c", "user.name=fixture", "-c", "user.email=fixture@example", "-c", "core.hooksPath=", "commit", "-q", "--file=-"], "a change over the pipe\n");
        assert!(answer.ok, "the commit went in: {}", answer.text);
        let shown = call(super::Tool::Git, &dir, &["log", "--format=%s"]);
        assert!(shown.text.contains("a change over the pipe"), "the message arrived as written: {}", shown.text);
    }
}
