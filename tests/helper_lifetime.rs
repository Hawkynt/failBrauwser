//! When the archive helper is allowed to end. The program starts it on first archive use, so
//! a helper that outlives its instance is a process the user sees and waits for; these
//! scenarios drive the real helper from outside and watch only what it answers and when it
//! goes away. Skipped (with a notice) when the helper has not been built.

use failbrauwser::archive::client::locate_helper;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

/// How long after its instance is gone the helper is expected to have gone itself. The watch
/// it keeps costs one wait call, so a second is generous; three covers a busy machine.
const ENDS_WITHIN: Duration = Duration::from_secs(3);

fn helper_exe() -> Option<PathBuf> {
    let exe = locate_helper();
    if !exe.is_file() {
        eprintln!("fb-archive helper not built, skipping ({})", exe.display());
        return None;
    }
    Some(exe)
}

/// A process that goes away on its own after about a second: stands in for the program
/// instance whose id the helper is told to watch.
fn instance_that_ends_by_itself() -> Child {
    #[cfg(windows)]
    let spawned = Command::new("ping").args(["-n", "2", "127.0.0.1"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    #[cfg(not(windows))]
    let spawned = Command::new("sleep").arg("1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    spawned.expect("the stand-in instance could not start")
}

/// The helper, spoken to exactly as the program speaks to it: one request, one answer.
fn started(extra_args: &[&str]) -> Option<(Child, ChildStdin, BufReader<ChildStdout>)> {
    let exe = helper_exe()?;
    let mut child = Command::new(&exe).args(extra_args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("the helper could not start");
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    Some((child, stdin, stdout))
}

/// `hello` and its answer: proves the pipe is open and the helper is working.
fn round_trip(child: &mut Child, stdin: &mut ChildStdin, stdout: &mut BufReader<ChildStdout>) {
    writeln!(stdin, "{{\"id\":1,\"cmd\":\"hello\"}}").unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(line.contains("\"version\""), "the helper did not answer the greeting: {line}");
    assert!(child.try_wait().unwrap().is_none(), "the helper ended before its instance did");
}

/// A helper told which process started it goes away with that process — even when the pipe
/// into it is still open, which is what leaves a helper behind after a killed instance.
#[test]
fn the_helper_ends_when_the_instance_that_started_it_goes_away() {
    let mut instance = instance_that_ends_by_itself();
    let parent = instance.id().to_string();
    let Some((mut child, mut stdin, mut stdout)) = started(&["--parent", &parent]) else { return };
    round_trip(&mut child, &mut stdin, &mut stdout);

    instance.wait().expect("the stand-in instance did not end");
    let deadline = Instant::now() + ENDS_WITHIN;
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = child.try_wait().unwrap().unwrap_or_else(|| panic!("the helper outlived the instance that started it: it is still running with its input pipe open"));
    drop(stdin);
    assert!(status.success(), "the helper ended by itself with {status}");
}

/// The plain case still holds: an instance that closes the helper's input ends the helper.
#[test]
fn the_helper_ends_when_the_instance_closes_its_input() {
    let Some((mut child, mut stdin, mut stdout)) = started(&[]) else { return };
    round_trip(&mut child, &mut stdin, &mut stdout);

    drop(stdin);
    let deadline = Instant::now() + ENDS_WITHIN;
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(child.try_wait().unwrap().is_some(), "the helper kept running after its input closed");
}
