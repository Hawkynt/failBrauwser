//! Asking a working copy's own client (M15). One call runs the program `Tool::cli` names, inside
//! the working copy, with the arguments `Command::runs` built; the answer keeps both streams of
//! the client's own words, in the order they arrived. Nothing here touches a repository's own
//! files, so whatever the tool decides about its data is what the window then shows — including
//! the tool's own complaint, which is the sentence a user can act on.

use super::Tool;
use std::io::{Read, Write};
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

/// How each of the five clients is kept from answering on its own: on a terminal, in a window of its
/// own, or through a pager standing between its words and the pipe reading them. Git Credential
/// Manager opens a credential window on a push, `svn` reads a passphrase straight off the terminal,
/// `less` swallows a log — on either platform. `ui::vcs::progress` puts this program's own askpass
/// answers on top of the table, so a question comes to our windows instead of a helper's.
pub const NON_INTERACTIVE: [(&str, &str); 7] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_PAGER", ""),
    ("PAGER", ""),
    ("HGPLAIN", "1"),
    ("GCM_INTERACTIVE", "never"),
    ("SVN_INTERACTIVE", "false"),
    ("SSH_ASKPASS", ""),
];

/// Puts that environment on one command; the locale each caller wants stays with the caller, because
/// the machine-parsed reads want `C` and the window that shows the words wants UTF-8.
pub fn non_interactive(c: &mut Command) {
    c.envs(NON_INTERACTIVE);
}

/// The clients this instance started and is still reading, by id. A read runs on a worker an ending
/// instance does not wait for, and the client it started then keeps running behind it — invisible,
/// at no CPU, holding a pipe nobody reads (see docs\TOOL-STARTS.md).
static CLIENTS: std::sync::LazyLock<std::sync::Mutex<Vec<u32>>> = std::sync::LazyLock::new(|| std::sync::Mutex::new(Vec::new()));

/// Keeps a client registered for exactly as long as its answer is being read, so that a read which
/// never comes back — because the instance ended under it — is the one that is left in the list.
struct Reading(u32);

impl Reading {
    /// Puts the client this call is about to read on the list, before its first byte is read: the
    /// whole time it is not off the list is the time an ending instance has a tool to end. A client
    /// that starts while the instance is ending is ended here instead, since nobody will read it.
    fn started(child: &std::process::Child) -> Self {
        let pid = child.id();
        let mut clients = CLIENTS.lock().unwrap();
        if ENDING.load(std::sync::atomic::Ordering::SeqCst) {
            drop(clients);
            end_process(pid);
        } else {
            clients.push(pid);
        }
        Self(pid)
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        CLIENTS.lock().unwrap().retain(|pid| *pid != self.0);
    }
}

/// Set once the instance has stopped its clients: after that a client of a read is ended as soon as
/// it is started, so nothing of ours is left behind by an instance on its way out.
static ENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The ids of the clients of this instance that are still being read.
#[cfg(test)]
fn our_clients() -> Vec<u32> {
    CLIENTS.lock().unwrap().clone()
}

/// Ends the clients this instance started and was still reading when it ended, and keeps any later
/// one from surviving its own start.
pub fn end_our_clients() {
    ENDING.store(true, std::sync::atomic::Ordering::SeqCst);
    for pid in std::mem::take(&mut *CLIENTS.lock().unwrap()) {
        end_process(pid);
    }
}

/// Ends the instance where an end never reaches `main`'s tail: the clients of reads still in flight
/// go first, so the rule does not depend on which way the instance ends. A self-test run ends the
/// process itself (`src\ui\selftest.rs`), which is where this is called; `tests\source_rules.rs`
/// keeps any other end in `src\ui` from coming back.
pub fn end_and_exit(code: i32) -> ! {
    end_our_clients();
    std::process::exit(code)
}

#[cfg(windows)]
fn end_process(pid: u32) {
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) }) else { return };
    unsafe {
        let _ = TerminateProcess(handle, 1);
        let _ = windows::Win32::Foundation::CloseHandle(handle);
    }
}

#[cfg(not(windows))]
fn end_process(pid: u32) {
    unsafe {
        let _ = libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

/// The handle of that job, reachable from every reading thread. A job handle is safe to use from any
/// of them — assigning a process to one is documented as thread-safe — but the `windows` crate spells
/// it as a raw pointer and cannot say so, which is what the two promises below are for.
#[cfg(windows)]
struct ReadJob(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
unsafe impl Send for ReadJob {}

#[cfg(windows)]
unsafe impl Sync for ReadJob {}

/// The job every reading client of this instance goes into: closing its handle ends what is inside,
/// and that handle goes away with the process however it ends — so a client is ended even by an end
/// that runs no ending code of ours at all, a crash or a `taskkill /F` that leaves the tree alone
/// (docs\TOOL-STARTS.md). Reads come here and nowhere else does: the acts the user asked for are left
/// to finish, which is why this instance never joins the job itself. The handle is never closed by
/// hand for exactly that reason — closing it early would end the reads that are still answering.
#[cfg(windows)]
static READ_JOB: std::sync::LazyLock<Option<ReadJob>> = std::sync::LazyLock::new(|| {
    use windows::Win32::System::JobObjects::{CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE};
    let Ok(job) = (unsafe { CreateJobObjectW(None, None) }) else { return None };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
    if unsafe { SetInformationJobObject(job, JobObjectExtendedLimitInformation, &limits as *const _ as *const core::ffi::c_void, size) }.is_err() {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(job);
        }
        return None;
    }
    Some(ReadJob(job))
});

/// Hands the client of a read to that job. Nothing is done when it cannot be — this process already
/// inside a job that forbids a nested one, for instance — because the id list is the rule again then,
/// which is exactly what it was before the job existed.
#[cfg(windows)]
fn keep_reading_client_with_us(child: &std::process::Child) {
    use windows::Win32::System::JobObjects::AssignProcessToJobObject;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};
    let assigned = match (READ_JOB.as_ref(), unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, child.id()) }) {
        (Some(job), Ok(handle)) => {
            let taken = unsafe { AssignProcessToJobObject(job.0, handle) }.is_ok();
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(handle);
            }
            taken
        }
        _ => false,
    };
    reading_announced(child, assigned);
}

/// No twin on the other side yet: there the ending call stays the whole rule, and a crash does not
/// reach it. Recorded in docs\TOOL-STARTS.md rather than papered over with a half-equivalent.
#[cfg(not(windows))]
fn keep_reading_client_with_us(child: &std::process::Child) {
    reading_announced(child, false);
}

/// Names the client a read started, for the test that ends this process without ending anything.
#[cfg(test)]
fn reading_announced(child: &std::process::Child, in_job: bool) {
    if std::env::var_os("FB_READ_ANNOUNCE").is_none() {
        return;
    }
    println!("READING {} job={}", child.id(), in_job);
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

#[cfg(not(test))]
fn reading_announced(_: &std::process::Child, _: bool) {}

/// What `GetExitCodeProcess` answers for a process that has not stopped yet (winbase's
/// `STILL_ACTIVE`; the `windows` crate 0.62 has no such constant to name it).
#[cfg(all(test, windows))]
const RUNS: u32 = 259;

/// Whether an id still names a running process — what the test below checks the ending against.
#[cfg(all(test, windows))]
fn process_runs(pid: u32) -> bool {
    use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else { return false };
    let mut code = 0u32;
    let answered = unsafe { GetExitCodeProcess(handle, &mut code) }.is_ok();
    unsafe {
        let _ = windows::Win32::Foundation::CloseHandle(handle);
    }
    answered && code == RUNS
}

#[cfg(all(test, not(windows)))]
fn process_runs(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Reads a started client to the end of both streams and waits for it, which is what `output()` does
/// — spelled out here because the call has to name the client's id while the answer is read, so that
/// `end_our_clients` reaches a read an ending instance left behind.
fn read_answer(mut child: std::process::Child) -> std::io::Result<std::process::Output> {
    // Into the job before anything else is done with it: from the moment the client exists, whatever
    // ends this process ends the client too, even where nothing of ours runs on the way out. The id
    // list stays as the second way, for an end that does get to run one.
    keep_reading_client_with_us(&child);
    let _reading = Reading::started(&child);
    let complaint = child.stderr.take().map(|mut stream| std::thread::spawn(move || { let mut bytes = Vec::new(); stream.read_to_end(&mut bytes).map(|_| bytes) }));
    let mut answer = Vec::new();
    if let Some(mut stream) = child.stdout.take() {
        stream.read_to_end(&mut answer)?;
    }
    let status = child.wait()?;
    let stderr = match complaint.map(|reader| reader.join()) {
        Some(Ok(bytes)) => bytes?,
        Some(Err(_)) => return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the reader of the client's complaint stopped before the client did")),
        None => Vec::new(),
    };
    Ok(std::process::Output { status, stdout: answer, stderr })
}

/// Starts the command and reads its answer — `Command::output` under another name, so that the error
/// of a program that is simply not there still reaches the caller's words about it, and so that a
/// caller which built its own `Command` still has the client it started registered: only a client in
/// the registry can be ended by `end_our_clients`.
pub fn read_of(command: &mut Command) -> std::io::Result<std::process::Output> {
    // The streams are named here rather than left to the caller: `Command::output` does the same, and
    // a client whose output is not a pipe answers into whoever started it — into this program's own
    // streams, for a windowed program. The six Git scenarios measured exactly that on 2026-11-17,
    // every window reading an empty answer.
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    read_answer(command.spawn()?)
}

/// The same, for a program named directly: the shape of the answer does not depend on which tool
/// a scenario is out to talk to.
pub fn ask(program: &str, dir: &Path, argv: &[impl AsRef<std::ffi::OsStr>]) -> Answer {
    let mut command = Command::new(program);
    command.current_dir(dir).args(argv).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command.env("LC_ALL", "C").env("LANG", "C");
    non_interactive(&mut command);
    // Every client here is a console program: without this each Repository command would bring up
    // a black window, since this program has no console for them to share.
    #[cfg(windows)]
    crate::win::console::no_window(&mut command);
    match read_of(&mut command) {
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
    match read_of(&mut command) {
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

    /// A client left to itself answers somewhere the window never asked: on the terminal, in a
    /// helper's own window, or into a pager. The table names each way the five clients do that, and a
    /// key must not appear twice, because the second entry silently wins.
    #[test]
    fn the_client_environment_leaves_nothing_to_answer_on_its_own() {
        let mut keys: Vec<&str> = NON_INTERACTIVE.iter().map(|(k, _)| *k).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), NON_INTERACTIVE.len(), "a key appears twice in the table");
        for want in ["GIT_TERMINAL_PROMPT", "GIT_PAGER", "PAGER", "HGPLAIN", "GCM_INTERACTIVE", "SVN_INTERACTIVE", "SSH_ASKPASS"] {
            assert!(NON_INTERACTIVE.iter().any(|(k, _)| *k == want), "{want} is how a client prompts or pages on its own");
        }
    }

    /// A repository `git` made here, or none where no git client runs.
    fn git_fixture() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
        let mut probe = Command::new("git");
        probe.arg("--version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        crate::win::console::no_window(&mut probe);
        if !probe.status().is_ok_and(|s| s.success()) {
            eprintln!("no git client on this machine, skipping the real-client scenarios");
            return None;
        }
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().to_path_buf();
        let run = |args: &[&str]| {
            let mut c = Command::new("git");
            c.current_dir(&dir).args(args).stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(windows)]
            crate::win::console::no_window(&mut c);
            assert!(c.status().expect("git could not run").success(), "git {args:?} did not answer");
        };
        run(&["init", "-q"]);
        Some((d, dir))
    }

    /// A read that is still in flight when the instance ends must not leave its client running
    /// behind it: invisible, at no CPU, holding handles and a pipe nobody reads (docs\TOOL-STARTS.md).
    /// The registry is what lets the ending reach a tool the reader has no handle for.
    #[test]
    fn a_client_still_reading_when_the_instance_ends_is_ended_there() {
        // A program that answers for far longer than the scenario waits for it: `ping` to the loopback
        // address on Windows, `sleep` where there is no `ping` with those arguments.
        let (program, argv): (&str, Vec<String>) = if cfg!(windows) { ("ping", vec!["-n".into(), "40".into(), "127.0.0.1".into()]) } else { ("sleep", vec!["40".into()]) };
        let named = format!("{program} {argv:?}");
        let before = our_clients();
        let reading = std::thread::spawn(move || ask(program, Path::new("."), &argv));
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let pid = loop {
            // Another scenario's client can be registered here too, so this one is the id that was
            // not in the list before this call started.
            if let Some(pid) = our_clients().into_iter().find(|p| !before.contains(p)) {
                break pid;
            }
            assert!(std::time::Instant::now() < until, "the call never registered the client it started, so nothing would end it");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        end_process(pid);
        let answer = reading.join().expect("the read did not come back when its client was ended");
        assert!(!answer.ok, "a client that was ended refuses the read it was answering: {:?}", answer.text);
        assert!(!our_clients().contains(&pid), "the client stayed registered after its read came back");
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while process_runs(pid) && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!process_runs(pid), "{named} of pid {pid} is still running, which is what an ending instance used to leave behind");
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

    /// An end that runs no ending code of ours at all — a crash, a `taskkill /F` left to take its own
    /// way — still has to end the client of a read, which is what the job is for (docs\TOOL-STARTS.md).
    /// The id list answers none of this, since the walk over it never runs. A copy of this test binary
    /// stands in for the instance: it reads a client that never answers and then waits, and the test
    /// ends that copy from outside — which is as close to a crash as an outside observer can get.
    /// Both waits stay short on purpose: a copy left behind by a panic here ends itself in a couple of
    /// minutes rather than holding a client open indefinitely.
    #[test]
    fn a_reading_client_is_gone_when_the_instance_is_killed_without_ending_anything() {
        if std::env::var_os("FB_KILLED_CHILD").is_some() {
            // The copy the test below started: read a client that outlives any waiting done here, and
            // stay alive until that test ends this process. Nothing on this path ends the client.
            std::thread::spawn(|| ask("ping", Path::new("."), &["-n", "60", "127.0.0.1"]));
            std::thread::sleep(std::time::Duration::from_secs(120));
            return;
        }
        if !cfg!(windows) {
            eprintln!("no job object on this platform: the ending call stays the whole rule there, which a crash never reaches");
            return;
        }
        let mut instance = Command::new(std::env::current_exe().expect("this test binary is the instance being ended"));
        instance
            .arg("vcs::run::tests::a_reading_client_is_gone_when_the_instance_is_killed_without_ending_anything")
            .arg("--exact")
            .arg("--nocapture")
            .env("FB_KILLED_CHILD", "1")
            .env("FB_READ_ANNOUNCE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped());
        // The rule of docs\TOOL-STARTS.md holds for this start too: the copy is a console program, and
        // a black window standing behind a scenario is what that rule exists to keep away.
        #[cfg(windows)]
        crate::win::console::no_window(&mut instance);
        let mut instance = instance.spawn().expect("the test could not start a copy of itself");
        // The copy names the client of its read and whether the job took it, and this is the only
        // reader of that pipe, so the line arrives here as it is written.
        let announced = {
            let answer = std::io::BufReader::new(instance.stdout.take().expect("the copy answers over a pipe"));
            std::io::BufRead::lines(answer).find_map(|line| line.ok().filter(|line| line.starts_with("READING ")))
        };
        let Some(line) = announced else {
            end_process(instance.id());
            panic!("the copy never named the client it started reading, so the job was never in play");
        };
        let mut fields = line.split_whitespace();
        let client = fields.nth(1).unwrap_or_default().parse::<u32>();
        let taken = fields.next().unwrap_or_default().to_owned();
        let Ok(client) = client else {
            end_process(instance.id());
            panic!("the copy named its client oddly: {line}");
        };
        eprintln!("the copy of this test binary announced its read as: {line}");
        if taken != "job=true" {
            // Nothing is provable here about a job this process was not allowed to have: a process
            // already inside a job that forbids a nested one leaves the id list as the only rule,
            // which is what the code degrades to rather than what it breaks.
            end_process(instance.id());
            end_process(client);
            eprintln!("skipped: this process could not put its own child into a job of its own ({line})");
            return;
        }
        end_process(instance.id());
        let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while process_runs(client) && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let (client_gone, instance_gone) = (!process_runs(client), !process_runs(instance.id()));
        if !client_gone {
            end_process(client); // nothing of this run stays behind, whatever the rule answered
        }
        end_process(instance.id());
        assert!(instance_gone, "the copy was still running, so nothing was measured here at all");
        assert!(client_gone, "the read client {client} outlived the instance that was ended without ending anything, so the read job did not take it");
    }
}
