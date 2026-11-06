//! Talks to the `fb-archive` helper process.
//!
//! The helper is started on first use and quits by itself after a period without requests,
//! so browsing plain directories never pays for it. Requests are serialized: one line of JSON
//! out, one line back.

use serde_json::{Value, json};
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long an unused helper stays alive.
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveError {
    /// `password`, `unsupported`, `notfound`, `permission`, `protocol`, `helper` or `error`.
    pub code: String,
    pub message: String,
}

impl ArchiveError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into() }
    }
    pub fn needs_password(&self) -> bool {
        self.code == "password"
    }
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ArchiveError {}

pub type Result<T> = std::result::Result<T, ArchiveError>;

thread_local! {
    static SINK: std::cell::RefCell<Option<Box<dyn FnMut(&Progress)>>> = const { std::cell::RefCell::new(None) };
}

/// Runs `f` with every helper request on this thread reporting its progress to `sink`.
pub fn with_progress<T>(sink: impl FnMut(&Progress) + 'static, f: impl FnOnce() -> T) -> T {
    SINK.with(|s| *s.borrow_mut() = Some(Box::new(sink)));
    let r = f();
    SINK.with(|s| *s.borrow_mut() = None);
    r
}

/// How far the helper is with the current request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    /// "extracting", "adding", "writing", "removing"
    pub phase: String,
}

struct Proc {
    child: Child,
    /// The helper's process id, readable without the lock (see [`Helper::abort`]); 0 once gone.
    pid: Arc<AtomicU32>,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Drop for Proc {
    fn drop(&mut self) {
        self.pid.store(0, Ordering::SeqCst);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct State {
    proc: Option<Proc>,
    last_use: Instant,
    reaper_running: bool,
}

/// A handle to the helper. Cheap to clone; all clones share one process.
#[derive(Clone)]
pub struct Helper {
    exe: PathBuf,
    shared: Arc<(Mutex<State>, Condvar)>,
    pid: Arc<AtomicU32>,
}

impl Helper {
    pub fn new(exe: PathBuf) -> Self {
        Self {
            exe,
            shared: Arc::new((
                Mutex::new(State { proc: None, last_use: Instant::now(), reaper_running: false }),
                Condvar::new(),
            )),
            pid: Arc::new(AtomicU32::new(0)),
        }
    }

    /// The process-wide helper, found next to the executable (see [`locate_helper`]).
    pub fn global() -> &'static Helper {
        static GLOBAL: OnceLock<Helper> = OnceLock::new();
        GLOBAL.get_or_init(|| Helper::new(locate_helper()))
    }

    pub fn is_running(&self) -> bool {
        self.shared.0.lock().unwrap().proc.is_some()
    }

    /// Stops the helper now; the next request starts it again.
    pub fn shutdown(&self) {
        let (lock, cv) = &*self.shared;
        let mut st = lock.lock().unwrap();
        st.proc = None;
        cv.notify_all();
    }

    /// Kills the helper in the middle of a request (a cancelled job): the request fails and
    /// the next one starts a fresh helper. Works while another thread waits for the answer.
    pub fn abort(&self) {
        let pid = self.pid.load(Ordering::SeqCst);
        if pid != 0 {
            kill_pid(pid);
        }
    }

    /// Every format the helper knows (the `formats` request), as JSON objects.
    pub fn formats(&self) -> Result<Vec<Value>> {
        let v = self.request(serde_json::json!({"cmd": "formats"}))?;
        Ok(v["formats"].as_array().cloned().unwrap_or_default())
    }

    /// Sends one request and waits for its answer. `req` must be a JSON object;
    /// the `id` field is filled in here.
    pub fn request(&self, req: Value) -> Result<Value> {
        // A job on this thread may be listening (see `with_progress`).
        SINK.with(|s| match s.borrow_mut().as_mut() {
            Some(sink) => self.request_with_progress(req, sink.as_mut()),
            None => self.request_with_progress(req, &mut |_| {}),
        })
    }

    /// Like [`Helper::request`], reporting the helper's progress lines on the way.
    pub fn request_with_progress(&self, mut req: Value, progress: &mut dyn FnMut(&Progress)) -> Result<Value> {
        let (lock, cv) = &*self.shared;
        let mut st = lock.lock().unwrap();
        if st.proc.is_none() {
            st.proc = Some(self.spawn()?);
        }
        if !st.reaper_running {
            st.reaper_running = true;
            self.start_reaper();
        }
        let result = {
            let proc = st.proc.as_mut().unwrap();
            proc.next_id += 1;
            let id = proc.next_id;
            req["id"] = json!(id);
            Self::roundtrip(proc, &req, id, progress)
        };
        if let Err(e) = &result {
            if e.code == "helper" {
                // The pipe is in an unknown state; start clean next time.
                st.proc = None;
            }
        }
        st.last_use = Instant::now();
        cv.notify_all();
        result
    }

    fn roundtrip(proc: &mut Proc, req: &Value, id: u64, progress: &mut dyn FnMut(&Progress)) -> Result<Value> {
        let broken = |e: std::io::Error| ArchiveError::new("helper", format!("archive helper failed: {e}"));
        let mut line = serde_json::to_string(req).map_err(|e| ArchiveError::new("protocol", e.to_string()))?;
        line.push('\n');
        proc.stdin.write_all(line.as_bytes()).map_err(broken)?;
        proc.stdin.flush().map_err(broken)?;
        let v: Value = loop {
            let mut answer = String::new();
            let n = proc.stdout.read_line(&mut answer).map_err(broken)?;
            if n == 0 {
                return Err(ArchiveError::new("helper", "archive helper exited unexpectedly"));
            }
            let v: Value = serde_json::from_str(&answer).map_err(|e| ArchiveError::new("helper", format!("bad answer from archive helper: {e}")))?;
            // Progress lines come before the answer.
            if let Some(p) = v.get("progress") {
                progress(&Progress {
                    done: p["done"].as_u64().unwrap_or(0),
                    total: p["total"].as_u64().unwrap_or(0),
                    phase: p["phase"].as_str().unwrap_or_default().to_string(),
                });
                continue;
            }
            break v;
        };
        if v["id"].as_u64() != Some(id) {
            return Err(ArchiveError::new("helper", "archive helper answered out of order"));
        }
        if v["ok"].as_bool() == Some(true) {
            Ok(v)
        } else {
            Err(ArchiveError::new(
                v["code"].as_str().unwrap_or("error"),
                v["error"].as_str().unwrap_or("unknown archive error"),
            ))
        }
    }

    fn spawn(&self) -> Result<Proc> {
        let mut child = Command::new(&self.exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // Keep the .NET runtime lean: the helper is short lived and single threaded.
            .env("DOTNET_gcServer", "0")
            .env("DOTNET_GCgen0size", "0x1000000")
            .env("DOTNET_TieredPGO", "0")
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    ArchiveError::new(
                        "helper",
                        "Archive support is not installed: the archive helper (fb-archive) is missing. \
                         Run install.sh again; it sets up the .NET SDK the helper is built with.",
                    )
                } else {
                    ArchiveError::new("helper", format!("cannot start archive helper {}: {e}", self.exe.display()))
                }
            })?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        self.pid.store(child.id(), Ordering::SeqCst);
        Ok(Proc { child, pid: self.pid.clone(), stdin, stdout, next_id: 0 })
    }

    /// One thread per running helper; it sleeps until the helper has been idle long
    /// enough, stops it and ends. Nothing wakes up while no helper runs.
    fn start_reaper(&self) {
        let shared = Arc::downgrade(&self.shared);
        std::thread::Builder::new()
            .name("fb-archive-reaper".into())
            .spawn(move || loop {
                let Some(shared) = shared.upgrade() else { return };
                let (lock, cv) = &*shared;
                let mut st = lock.lock().unwrap();
                let idle = st.last_use.elapsed();
                if st.proc.is_none() || idle >= IDLE_TIMEOUT {
                    st.proc = None;
                    st.reaper_running = false;
                    return;
                }
                drop(cv.wait_timeout(st, IDLE_TIMEOUT - idle).unwrap());
            })
            .expect("spawn reaper thread");
    }
}

/// The helper's executable name(s) to look for.
#[cfg(windows)]
fn helper_names() -> [&'static str; 2] {
    ["fb-archive.exe", "fb-archive"]
}

#[cfg(not(windows))]
fn helper_names() -> [&'static str; 1] {
    ["fb-archive"]
}

/// Ends the helper process by id (see `Helper::abort`).
#[cfg(unix)]
fn kill_pid(pid: u32) {
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

#[cfg(windows)]
fn kill_pid(pid: u32) {
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    let Ok(h) = (unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) }) else { return };
    unsafe {
        let _ = TerminateProcess(h, 1);
        let _ = windows::Win32::Foundation::CloseHandle(h);
    }
}

/// Finds the helper binary: `$FB_ARCHIVE_HELPER`, next to our executable,
/// `../lib/failbrauwser/`, or the development build output.
pub fn locate_helper() -> PathBuf {
    if let Some(p) = std::env::var_os("FB_ARCHIVE_HELPER") {
        return PathBuf::from(p);
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in helper_names() {
                candidates.push(dir.join(name));
                candidates.push(dir.join("../lib/failbrauwser").join(name));
            }
        }
    }
    for name in helper_names() {
        candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("target/helper").join(name));
    }
    candidates
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .unwrap_or_else(|| PathBuf::from("fb-archive"))
}
