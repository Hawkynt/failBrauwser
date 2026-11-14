//! TortoiseGit's "Git Command Progress" window: runs git commands one after the other,
//! shows each command and what it prints, a progress bar for transfers, and at the end
//! "Success (… ms @ …)" in green or what went wrong in red, with buttons for what can come
//! next (Push after a commit, …).

use super::super::window::Window;
use super::dialog;
use gtk::prelude::*;
use gtk::glib;
use std::cell::Cell;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// One client command: the client, its arguments and what it reads from its input.
#[derive(Clone, Debug)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<String>,
}

/// A git command.
pub fn cmd<S: AsRef<str>>(args: &[S]) -> Cmd {
    Cmd { program: "git".into(), args: args.iter().map(|a| a.as_ref().to_string()).collect(), stdin: None }
}

/// A command of a tool's own client.
pub fn tool_cmd(tool: failbrauwser::vcs::Tool, args: &[String]) -> Cmd {
    Cmd { program: tool.cli().into(), args: args.to_vec(), stdin: None }
}

/// The client calls of an item, one command each.
pub fn tool_cmds(tool: failbrauwser::vcs::Tool, runs: Vec<Vec<String>>) -> Vec<Cmd> {
    runs.iter().map(|a| tool_cmd(tool, a)).collect()
}

impl Cmd {
    pub fn input(mut self, text: &str) -> Cmd {
        self.stdin = Some(text.to_string());
        self
    }
    /// How TortoiseGit shows it: "git commit -F -", names with spaces quoted.
    pub fn display(&self) -> String {
        let args: Vec<String> = self.args.iter().map(|a| if a.contains(' ') || a.is_empty() { format!("\"{a}\"") } else { a.clone() }).collect();
        format!("{} {}", self.program, args.join(" "))
    }
}

/// A button shown when everything went well, and what it does.
pub type Post = (String, Rc<dyn Fn(&Window)>);

/// What running the commands brings.
pub enum Event {
    /// The command with this index starts.
    Start(usize),
    Out(String),
    /// A line git keeps rewriting (progress of a transfer).
    Progress(String),
    /// All ran (Some(0)) or one failed with this exit code (None: killed).
    Done(Option<i32>),
}

/// "Success", or why not, as TortoiseGit words it, naming the client that ran.
pub fn outcome(program: &str, code: Option<i32>, aborted: bool) -> String {
    match code {
        Some(0) => "Success".into(),
        Some(c) => format!("{program} did not exit cleanly (exit code {c})"),
        None if aborted => "Aborted by user".into(),
        None => format!("{program} was terminated"),
    }
}

/// The tool whose client a command runs, for the window's title ("Git Command Progress").
fn tool_label(cmds: &[Cmd]) -> &'static str {
    let program = cmds.first().map(|c| c.program.as_str()).unwrap_or("git");
    failbrauwser::vcs::Tool::all().iter().find(|t| t.cli() == program).map(|t| t.label()).unwrap_or("Git")
}

/// Runs `cmds` one after the other in `dir` on a worker thread, until one fails; the
/// running child is kept in `slot` (to abort it).
pub fn execute(dir: &Path, cmds: Vec<Cmd>, slot: Arc<Mutex<Option<Child>>>) -> async_channel::Receiver<Event> {
    let (tx, rx) = async_channel::unbounded::<Event>();
    let dir = dir.to_path_buf();
    std::thread::Builder::new()
        .name("fb-git-run".into())
        .spawn(move || {
            for (i, c) in cmds.iter().enumerate() {
                let _ = tx.send_blocking(Event::Start(i));
                let code = run_one(&dir, c, &slot, &tx);
                if code != Some(0) || i + 1 == cmds.len() {
                    let _ = tx.send_blocking(Event::Done(code));
                    return;
                }
            }
            let _ = tx.send_blocking(Event::Done(Some(0)));
        })
        .expect("spawn git");
    rx
}

/// The environment the commands run in: no terminal prompts; passwords and passphrases
/// are asked for in a window of ours.
pub fn prepare(c: &mut std::process::Command) {
    c.env("GIT_TERMINAL_PROMPT", "0").env("GIT_LITERAL_PATHSPECS", "1").env("HGPLAIN", "1").env("LC_ALL", "C.UTF-8").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Ok(exe) = std::env::current_exe() {
        c.env("GIT_ASKPASS", &exe).env("SSH_ASKPASS", &exe).env("SSH_ASKPASS_REQUIRE", "force").env(super::askpass::ENV, "1");
    }
}

pub struct Progress {
    pub dialog: gtk::Dialog,
    buffer: gtk::TextBuffer,
    view: gtk::TextView,
    bar: gtk::ProgressBar,
    line: gtk::Label,
    result: gtk::Label,
    posts: gtk::Box,
    close: gtk::Button,
    child: Arc<Mutex<Option<Child>>>,
    pub finished: Cell<Option<bool>>,
    aborted: Cell<bool>,
}

/// Runs `cmds` in `dir`, shown in a progress window titled after `path`. `done` hears
/// whether all of them succeeded; `posts` are offered afterwards when they did.
pub fn run(w: &Window, dir: &Path, path: &Path, cmds: Vec<Cmd>, posts: Vec<Post>, done: impl Fn(&Window, bool) + 'static) -> Rc<Progress> {
    let d = dialog::new(w, &dialog::title(path, &format!("{} Command Progress", tool_label(&cmds))), "tgit");
    d.set_default_size(720, 420);
    let area = d.content_area();
    let (sw, view) = dialog::text_view("git-output", false);
    view.set_wrap_mode(gtk::WrapMode::None);
    let buffer = view.buffer().expect("buffer");
    let tags = buffer.tag_table().expect("tags");
    for (name, props) in [("cmd", ("#1d4fbf", 700)), ("ok", ("#1f8f2f", 700)), ("fail", ("#c62020", 700))] {
        let t = gtk::TextTag::new(Some(name));
        t.set_foreground(Some(props.0));
        t.set_weight(props.1);
        tags.add(&t);
    }
    sw.set_vexpand(true);
    area.pack_start(&sw, true, true, 0);
    let line = dialog::label("");
    line.set_widget_name("git-progress-line");
    line.set_ellipsize(gtk::pango::EllipsizeMode::End);
    area.pack_start(&line, false, false, 0);
    let bar = gtk::ProgressBar::new();
    area.pack_start(&bar, false, false, 0);
    // What the output ends with, for tests (TortoiseGit shows it in the output only).
    let result = dialog::label("");
    result.set_widget_name("git-result");
    result.set_no_show_all(true);
    area.pack_start(&result, false, false, 0);
    // Buttons for what comes next on the left, Abort/Close on the right.
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let posts_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bottom.pack_start(&posts_box, false, false, 0);
    let close = gtk::Button::with_label("Abort");
    close.set_widget_name("git-close");
    bottom.pack_end(&close, false, false, 0);
    area.pack_end(&bottom, false, false, 0);
    let p = Rc::new(Progress {
        dialog: d.clone(),
        buffer,
        view,
        bar,
        line,
        result,
        posts: posts_box,
        close: close.clone(),
        child: Arc::new(Mutex::new(None)),
        finished: Cell::new(None),
        aborted: Cell::new(false),
    });
    dialog::keep_alive(&d, p.clone());
    let pw = Rc::downgrade(&p);
    close.connect_clicked(move |b| {
        let Some(p) = pw.upgrade() else { return };
        if p.finished.get().is_some() {
            unsafe { p.dialog.destroy() };
            return;
        }
        p.aborted.set(true);
        if let Some(c) = p.child.lock().unwrap().as_mut() {
            let _ = c.kill();
        }
        b.set_sensitive(false);
    });
    let pw = Rc::downgrade(&p);
    d.connect_delete_event(move |_, _| {
        // Closing while running aborts first.
        if let Some(p) = pw.upgrade() {
            if p.finished.get().is_none() {
                p.close.clicked();
                return glib::Propagation::Stop;
            }
        }
        glib::Propagation::Proceed
    });
    d.show_all();
    let rx = execute(&dir.to_path_buf(), cmds.clone(), p.child.clone());
    let ww = Rc::downgrade(&w.me());
    let p2 = p.clone();
    let started = Instant::now();
    glib::spawn_future_local(async move {
        let mut ok = false;
        while let Ok(ev) = rx.recv().await {
            match ev {
                Event::Start(i) => p2.append(&format!("{}{}\n", if i == 0 { "" } else { "\n" }, cmds[i].display()), Some("cmd")),
                Event::Out(s) => p2.append(&s, None),
                Event::Progress(s) => p2.show_progress(&s),
                Event::Done(code) => {
                    ok = code == Some(0);
                    let program = cmds.first().map(|c| c.program.clone()).unwrap_or_default();
                    p2.finish(ok, &outcome(&program, code, p2.aborted.get()), started);
                }
            }
        }
        let Some(w) = ww.upgrade() else { return };
        if ok {
            for (label, f) in posts {
                let b = gtk::Button::with_label(&label);
                let ww = Rc::downgrade(&w.me());
                let d = p2.dialog.clone();
                b.connect_clicked(move |_| {
                    if let Some(w) = ww.upgrade() {
                        unsafe { d.destroy() };
                        f(&w);
                    }
                });
                p2.posts.pack_start(&b, false, false, 0);
                b.show();
            }
        }
        super::overlay::refresh_all(&w);
        done(&w, ok);
    });
    p
}

/// Runs one command, sending what it prints; its exit code (None when killed).
fn run_one(dir: &Path, c: &Cmd, slot: &Arc<Mutex<Option<Child>>>, tx: &async_channel::Sender<Event>) -> Option<i32> {
    let mut command = std::process::Command::new(&c.program);
    command.current_dir(dir).args(&c.args);
    prepare(&mut command);
    let mut child = match command.spawn() {
        Ok(ch) => ch,
        Err(e) => {
            let _ = tx.send_blocking(Event::Out(format!("{} cannot be started: {e}\n", c.program)));
            return Some(127);
        }
    };
    let mut stdin = child.stdin.take();
    let input = c.stdin.clone();
    let writer = std::thread::spawn(move || {
        if let (Some(s), Some(text)) = (stdin.as_mut(), input) {
            let _ = s.write_all(text.as_bytes());
        }
        drop(stdin);
    });
    let readers: Vec<_> = [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)]
        .into_iter()
        .flatten()
        .map(|r| {
            let tx = tx.clone();
            std::thread::spawn(move || pump(r, &tx))
        })
        .collect();
    *slot.lock().unwrap() = Some(child);
    let _ = writer.join();
    for r in readers {
        let _ = r.join();
    }
    let status = slot.lock().unwrap().take().and_then(|mut ch| ch.wait().ok());
    status.and_then(|s| s.code())
}

/// Splits output into lines; lines ended by '\r' are progress, shown but not kept.
fn pump(mut r: Box<dyn Read + Send>, tx: &async_channel::Sender<Event>) {
    let mut buf = [0u8; 8192];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &b in &buf[..n] {
            match b {
                b'\n' => {
                    pending.push(b'\n');
                    let _ = tx.send_blocking(Event::Out(String::from_utf8_lossy(&pending).into_owned()));
                    pending.clear();
                }
                b'\r' => {
                    let _ = tx.send_blocking(Event::Progress(String::from_utf8_lossy(&pending).into_owned()));
                    pending.clear();
                }
                _ => pending.push(b),
            }
        }
    }
    if !pending.is_empty() {
        pending.push(b'\n');
        let _ = tx.send_blocking(Event::Out(String::from_utf8_lossy(&pending).into_owned()));
    }
}

/// The percentage in a progress line ("Receiving objects:  45% (9/20)").
pub fn percent(line: &str) -> Option<f64> {
    let i = line.find('%')?;
    let digits: String = line[..i].chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
    digits.parse::<f64>().ok().map(|v| (v / 100.0).clamp(0.0, 1.0))
}

impl Progress {
    fn append(&self, text: &str, tag: Option<&str>) {
        let mut end = self.buffer.end_iter();
        match tag {
            Some(t) => {
                let start = end.offset();
                self.buffer.insert(&mut end, text);
                let s = self.buffer.iter_at_offset(start);
                self.buffer.apply_tag_by_name(t, &s, &self.buffer.end_iter());
            }
            None => self.buffer.insert(&mut end, text),
        }
        // Lines git printed as progress end up here too, finished.
        if let Some(f) = percent(text) {
            self.bar.set_fraction(f);
        }
        if let Some(mark) = self.buffer.create_mark(None, &self.buffer.end_iter(), false) {
            self.view.scroll_mark_onscreen(&mark);
            self.buffer.delete_mark(&mark);
        }
    }

    fn show_progress(&self, line: &str) {
        self.line.set_text(line.trim());
        match percent(line) {
            Some(f) => self.bar.set_fraction(f),
            None => self.bar.pulse(),
        }
    }

    pub fn finish(&self, ok: bool, what: &str, started: Instant) {
        let ms = started.elapsed().as_millis();
        let now = glib::DateTime::now_local().ok().and_then(|d| d.format("%Y-%m-%d %H:%M:%S").ok()).map(|s| s.to_string()).unwrap_or_default();
        let text = format!("{what} ({ms} ms @ {now})");
        self.append(&format!("\n{text}\n"), Some(if ok { "ok" } else { "fail" }));
        self.result.set_markup(&format!("<b><span foreground=\"{}\">{}</span></b>", if ok { "#1f8f2f" } else { "#c62020" }, glib::markup_escape_text(&text)));
        self.line.set_text("");
        if ok {
            self.bar.set_fraction(1.0);
        }
        self.finished.set(Some(ok));
        self.close.set_label("Close");
        self.close.set_sensitive(true);
        self.close.grab_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_of_progress_lines() {
        assert_eq!(percent("Receiving objects:  45% (9/20)"), Some(0.45));
        assert_eq!(percent("Writing objects: 100% (3/3), done."), Some(1.0));
        assert_eq!(percent("Counting objects: 3"), None);
    }

    #[test]
    fn commands_shown_like_tortoisegit() {
        assert_eq!(cmd(&["commit", "-F", "-"]).display(), "git commit -F -");
        assert_eq!(cmd(&["add", "-A", "--", "a b.txt"]).display(), "git add -A -- \"a b.txt\"");
    }
}
