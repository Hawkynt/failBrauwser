//! Help > Check for Updates: tells whether GitHub has a newer release and installs either
//! that release's ready-built package or the current source built here (the source is
//! deleted afterwards). A script started on its own does the work, so it lives on while the
//! new version replaces this one; its output shows in the window.

use super::window::Window;
use failbrauwser::update::{self, Release};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

pub struct Updater {
    pub window: gtk::Window,
    status: gtk::Label,
    release: gtk::RadioButton,
    source: gtk::RadioButton,
    target: gtk::Label,
    log: gtk::TextView,
    log_scroll: gtk::ScrolledWindow,
    start: gtk::Button,
    newest: RefCell<Option<Release>>,
    /// Where it is installed, if this copy can update itself.
    prefix: Option<PathBuf>,
    running: RefCell<Option<std::process::Child>>,
    weak: RefCell<Weak<Updater>>,
}

thread_local! {
    static OPEN: RefCell<Option<Rc<Updater>>> = const { RefCell::new(None) };
}

/// The update window shown (tests).
pub fn current() -> Option<Rc<Updater>> {
    OPEN.with(|o| o.borrow().clone())
}

fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Where this copy is installed (`FB_UPDATE_PREFIX` in tests), if it can update itself:
/// installed under a `bin` folder it may write to.
fn own_prefix() -> Result<PathBuf, String> {
    let prefix = match std::env::var_os("FB_UPDATE_PREFIX") {
        Some(p) => PathBuf::from(p),
        None => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            update::prefix_of(&exe).ok_or_else(|| format!("This copy runs from {}, not from an installation. Update it with git pull and ./install.sh.", exe.display()))?
        }
    };
    let writable = failbrauwser::osprim::can_write(&prefix.join("bin"));
    if !writable {
        return Err(format!(
            "failBrauwser is installed for everyone in {}. Update it from a terminal: download the release or the source and run ./install.sh --system.",
            prefix.display()
        ));
    }
    Ok(prefix)
}

pub fn show(w: &Window) {
    if let Some(u) = current() {
        u.window.present();
        return;
    }
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Update failBrauwser");
    window.set_transient_for(Some(&w.win));
    window.set_default_size(620, -1);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_border_width(14);
    let current_label = gtk::Label::new(Some(&format!("This is failBrauwser {}.", version())));
    current_label.set_xalign(0.0);
    let status = gtk::Label::new(Some("Asking GitHub for the newest release…"));
    status.set_xalign(0.0);
    status.set_line_wrap(true);
    status.set_widget_name("update-status");
    let release = gtk::RadioButton::with_mnemonic("The newest _release from GitHub, ready built");
    let source = gtk::RadioButton::with_mnemonic_from_widget(&release, "Build from the current _source (a few minutes; needs Rust, GTK's development files and a C compiler; the source is deleted afterwards)");
    if let Some(l) = source.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
        l.set_line_wrap(true);
    }
    let target = gtk::Label::new(None);
    target.set_xalign(0.0);
    target.set_line_wrap(true);
    target.style_context().add_class("dim-label");
    let log = gtk::TextView::new();
    log.set_editable(false);
    log.set_monospace(true);
    log.set_wrap_mode(gtk::WrapMode::WordChar);
    let log_scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    log_scroll.set_min_content_height(220);
    log_scroll.set_shadow_type(gtk::ShadowType::In);
    log_scroll.add(&log);
    log_scroll.set_no_show_all(true);
    let close = gtk::Button::with_mnemonic("_Close");
    let start = gtk::Button::with_mnemonic("_Update");
    start.style_context().add_class("suggested-action");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.pack_end(&start, false, false, 0);
    buttons.pack_end(&close, false, false, 0);
    for wdg in [current_label.upcast_ref::<gtk::Widget>(), status.upcast_ref(), release.upcast_ref(), source.upcast_ref(), target.upcast_ref(), log_scroll.upcast_ref(), buttons.upcast_ref()] {
        root.pack_start(wdg, false, false, 0);
    }
    root.set_child_packing(&log_scroll, true, true, 0, gtk::PackType::Start);
    window.add(&root);

    let prefix = own_prefix();
    let u = Rc::new(Updater {
        window: window.clone(),
        status,
        release,
        source,
        target,
        log,
        log_scroll,
        start,
        newest: RefCell::new(None),
        prefix: prefix.as_ref().ok().cloned(),
        running: RefCell::new(None),
        weak: RefCell::new(Weak::new()),
    });
    *u.weak.borrow_mut() = Rc::downgrade(&u);
    match &prefix {
        Ok(p) => {
            let opts = update::install_options(&glib::user_config_dir().join("failbrauwser").join("install-options"));
            let opts = if opts.is_empty() { "the usual options".to_string() } else { opts.join(" ") };
            u.target.set_text(&format!("Installs into {} with {opts}, like the last install. failBrauwser restarts when it is done.", p.display()));
        }
        Err(why) => u.target.set_text(why),
    }
    let w2 = window.clone();
    close.connect_clicked(move |_| w2.close());
    let weak = Rc::downgrade(&u);
    u.start.connect_clicked(move |_| {
        if let Some(u) = weak.upgrade() {
            u.run();
        }
    });
    for b in [&u.release, &u.source] {
        let weak = Rc::downgrade(&u);
        b.connect_toggled(move |_| {
            if let Some(u) = weak.upgrade() {
                u.update_buttons();
            }
        });
    }
    window.connect_destroy(|_| OPEN.with(|o| *o.borrow_mut() = None));
    OPEN.with(|o| *o.borrow_mut() = Some(u.clone()));
    window.show_all();
    u.update_buttons();
    u.check();
}

/// `text` as given by `url` (curl, else wget).
fn fetch(url: &str) -> Result<String, String> {
    let run = |cmd: &str, args: &[&str]| std::process::Command::new(cmd).args(args).output();
    let out = run("curl", &["-fsSL", "--max-time", "20", "-H", "Accept: application/vnd.github+json", url]).or_else(|_| run("wget", &["-qO-", "--timeout=20", url])).map_err(|_| "Neither curl nor wget is installed.".to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

impl Updater {
    fn me(&self) -> Rc<Updater> {
        self.weak.borrow().upgrade().expect("update window alive")
    }

    /// Asks GitHub for the newest release.
    fn check(&self) {
        let me = self.me();
        glib::spawn_future_local(async move {
            let url = update::release_api();
            let res = gio::spawn_blocking(move || fetch(&url)).await.unwrap_or_else(|_| Err("failed".into()));
            match res.as_deref().map(update::parse_release) {
                Ok(Some(r)) => {
                    let when = r.published.split('T').next().unwrap_or_default().to_string();
                    let newer = update::is_newer(&r.tag, version());
                    let mut text = if newer { format!("{} ({when}) is newer.", r.name) } else { format!("You have the newest release ({}).", r.name) };
                    if r.package.is_none() {
                        text.push_str(" It has no ready-built package for this computer: build from the source.");
                    }
                    me.status.set_text(&text);
                    if r.package.is_none() || !newer {
                        me.source.set_active(true);
                    }
                    *me.newest.borrow_mut() = Some(r);
                }
                Ok(None) => me.status.set_text("GitHub answered with something else than a release."),
                Err(e) => me.status.set_text(&format!("GitHub could not be asked: {e}")),
            }
            me.update_buttons();
        });
    }

    fn update_buttons(&self) {
        let package = self.newest.borrow().as_ref().and_then(|r| r.package.clone());
        self.release.set_sensitive(package.is_some());
        if package.is_none() && self.release.is_active() {
            self.source.set_active(true);
        }
        let ready = self.prefix.is_some() && self.running.borrow().is_none() && (self.source.is_active() || package.is_some());
        self.start.set_sensitive(ready);
    }

    /// Writes the script, starts it on its own, and shows what it says.
    pub fn run(&self) {
        let Some(prefix) = self.prefix.clone() else { return };
        let package = if self.release.is_active() { self.newest.borrow().as_ref().and_then(|r| r.package.clone()) } else { None };
        let opts = update::install_options(&glib::user_config_dir().join("failbrauwser").join("install-options"));
        let cache = glib::user_cache_dir();
        let script = update::script(package.as_deref(), &prefix, &opts, &cache);
        let dir = cache.join("failbrauwser");
        let res = (|| -> std::io::Result<std::process::Child> {
            std::fs::create_dir_all(&dir)?;
            let path = dir.join("update.sh");
            std::fs::write(&path, script)?;
            let log = std::fs::File::create(dir.join("update.log"))?;
            // A session of its own: it lives on when the new version replaces this one.
            std::process::Command::new("setsid").arg("sh").arg(&path).stdin(std::process::Stdio::null()).stdout(log.try_clone()?).stderr(log).spawn()
        })();
        let child = match res {
            Ok(c) => c,
            Err(e) => {
                self.status.set_text(&format!("The update could not start: {e}"));
                return;
            }
        };
        *self.running.borrow_mut() = Some(child);
        self.status.set_text(if package.is_some() { "Downloading and installing the release…" } else { "Downloading and building the source… (this takes a few minutes)" });
        self.log_scroll.show_all();
        self.update_buttons();
        let me = self.me();
        let log_path = dir.join("update.log");
        glib::timeout_add_local(std::time::Duration::from_millis(300), move || {
            let text = std::fs::read_to_string(&log_path).unwrap_or_default();
            let buf = me.log.buffer().expect("text view has a buffer");
            if buf.char_count() as usize != text.chars().count() {
                buf.set_text(&text);
                let mut end = buf.end_iter();
                me.log.scroll_to_iter(&mut end, 0.0, false, 0.0, 1.0);
            }
            let finished = me.running.borrow_mut().as_mut().and_then(|c| c.try_wait().ok().flatten());
            let Some(exit) = finished else { return glib::ControlFlow::Continue };
            me.running.borrow_mut().take();
            me.status.set_text(if exit.success() && text.contains("UPDATE FINISHED") {
                "Updated. The new version starts in a moment."
            } else {
                "The update failed; what went wrong is in the log above. Nothing was changed where it stopped before installing."
            });
            me.update_buttons();
            glib::ControlFlow::Break
        });
    }

    pub fn status_text(&self) -> String {
        self.status.text().to_string()
    }

    pub fn is_checking(&self) -> bool {
        self.status.text().starts_with("Asking")
    }

    pub fn is_running(&self) -> bool {
        self.running.borrow().is_some()
    }

    pub fn choose(&self, release: bool) {
        if release { self.release.set_active(true) } else { self.source.set_active(true) }
    }
}
