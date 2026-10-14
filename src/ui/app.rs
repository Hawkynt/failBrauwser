//! The application: one process per session, windows on demand.
//!
//! A second `failbrauwser` invocation does not initialise GTK at all: GApplication forwards
//! its command line over D-Bus to the running instance and exits, so a new window appears
//! within milliseconds. With the `daemon` setting the first instance stays resident after
//! its last window closes (like `thunar --daemon`), releasing caches while idle.

use super::window::Window;
use failbrauwser::config::Settings;
use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

pub const APP_ID: &str = "org.failbrauwser.FailBrauwser";

/// Our own icon once installed, the theme's file manager icon otherwise.
pub fn icon_name() -> String {
    let has_own = gtk::IconTheme::default().is_some_and(|t| t.has_icon(APP_ID));
    if has_own { APP_ID.to_string() } else { "system-file-manager".to_string() }
}

pub struct AppCtx {
    pub app: gtk::Application,
    pub settings: Rc<RefCell<Settings>>,
    pub jobs: Rc<super::jobs::Jobs>,
    windows: RefCell<Vec<Rc<Window>>>,
    hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    save_pending: Cell<bool>,
    weak: RefCell<Weak<AppCtx>>,
}

impl AppCtx {
    pub fn me(&self) -> Rc<AppCtx> {
        self.weak.borrow().upgrade().expect("app alive")
    }

    pub fn windows(&self) -> Vec<Rc<Window>> {
        self.windows.borrow().clone()
    }

    /// Runs `f` for every open window (after a global setting changed).
    pub fn for_each_window(&self, f: impl Fn(&Rc<Window>)) {
        for w in self.windows() {
            f(&w);
        }
    }

    pub fn open_window(&self, locations: Vec<Location>) -> Rc<Window> {
        let locations = if locations.is_empty() { vec![Location::home()] } else { locations };
        let w = Window::new(&self.me(), locations);
        self.windows.borrow_mut().push(w.clone());
        w.win.present();
        w
    }

    pub fn window_closed(&self, closed: &Rc<Window>) {
        self.windows.borrow_mut().retain(|w| !Rc::ptr_eq(w, closed));
        self.save_settings_now();
        if self.windows().is_empty() {
            self.go_idle();
        }
    }

    /// Nothing is shown any more: give memory back while we wait for the next window.
    fn go_idle(&self) {
        super::util::clear_caches();
        failbrauwser::archive::Helper::global().shutdown();
        unsafe {
            libc::malloc_trim(0);
        }
    }

    /// Saves settings shortly after a change, once for a burst of changes.
    pub fn save_settings_soon(&self) {
        if self.save_pending.replace(true) {
            return;
        }
        let weak = self.weak.borrow().clone();
        glib::timeout_add_local_once(std::time::Duration::from_secs(1), move || {
            if let Some(a) = weak.upgrade() {
                a.save_settings_now();
            }
        });
    }

    pub fn save_settings_now(&self) {
        self.save_pending.set(false);
        if let Err(e) = self.settings.borrow().save() {
            eprintln!("failbrauwser: cannot save settings: {e}");
        }
    }

    /// Applies the daemon setting: hold the application so it outlives its windows.
    pub fn apply_daemon(&self) {
        let want = self.settings.borrow().daemon;
        let mut hold = self.hold.borrow_mut();
        if want && hold.is_none() {
            *hold = Some(self.app.hold());
        } else if !want {
            // Dropping the guard lets the application exit with its last window.
            *hold = None;
        }
    }
}

/// Locations to open, network addresses to mount first (`smb://…`), `--daemon`, `--quit`.
fn parse_args(args: &[std::ffi::OsString], cwd: Option<PathBuf>) -> (Vec<Location>, Vec<String>, bool, bool) {
    let mut locations = Vec::new();
    let mut remote = Vec::new();
    let mut daemon = false;
    let mut quit = false;
    for a in args.iter().skip(1) {
        let s = a.to_string_lossy();
        match s.as_ref() {
            "--daemon" | "-d" => daemon = true,
            "--quit" | "-q" => quit = true,
            _ if s.starts_with('-') => {}
            _ if super::util::is_remote_uri(&s) => remote.push(s.to_string()),
            _ => {
                let text = if s.starts_with('/') || s.contains("://") || s.starts_with('~') || s.starts_with("drives:") {
                    s.to_string()
                } else {
                    cwd.clone().unwrap_or_default().join(s.as_ref()).to_string_lossy().into_owned()
                };
                match Location::parse(&text, failbrauwser::archive::is_browsable_name) {
                    Some(Location::Archive(a)) if a.nested.is_empty() && a.inner.is_empty() && !failbrauwser::archive::is_browsable_name(&a.file.to_string_lossy()) => {
                        // A plain file: show the folder it is in.
                        if let Some(dir) = a.file.parent() {
                            locations.push(Location::Dir(dir.to_path_buf()));
                        }
                    }
                    Some(l) => locations.push(l),
                    None => eprintln!("failbrauwser: cannot open {text}"),
                }
            }
        }
    }
    (locations, remote, daemon, quit)
}

pub fn run() -> glib::ExitCode {
    let app = gtk::Application::new(Some(APP_ID), gio::ApplicationFlags::HANDLES_COMMAND_LINE);
    let ctx: Rc<RefCell<Option<Rc<AppCtx>>>> = Rc::new(RefCell::new(None));

    let c = ctx.clone();
    app.connect_startup(move |app| {
        gtk::Window::set_default_icon_name(&icon_name());
        super::style::install();
        let settings = Rc::new(RefCell::new(Settings::load()));
        let actx = Rc::new(AppCtx {
            app: app.clone(),
            settings,
            jobs: super::jobs::Jobs::new(),
            windows: RefCell::new(Vec::new()),
            hold: RefCell::new(None),
            save_pending: Cell::new(false),
            weak: RefCell::new(Weak::new()),
        });
        *actx.weak.borrow_mut() = Rc::downgrade(&actx);
        super::actions::install_app_actions(&actx);
        let weak = Rc::downgrade(&actx);
        super::clipboard::connect_changed(move || {
            if let Some(a) = weak.upgrade() {
                a.for_each_window(|w| {
                    for p in w.panes() {
                        p.mark_cut();
                    }
                });
            }
        });
        actx.apply_daemon();
        *c.borrow_mut() = Some(actx);
    });

    let c = ctx.clone();
    app.connect_command_line(move |_app, cmdline| {
        let Some(actx) = c.borrow().clone() else { return glib::ExitCode::FAILURE };
        let (locations, remote, daemon, quit) = parse_args(&cmdline.arguments(), cmdline.cwd());
        if quit {
            actx.save_settings_now();
            actx.app.quit();
            return glib::ExitCode::SUCCESS;
        }
        if daemon {
            // Started in the background (autostart): stay resident, show nothing.
            let mut hold = actx.hold.borrow_mut();
            if hold.is_none() {
                *hold = Some(actx.app.hold());
            }
            return glib::ExitCode::SUCCESS;
        }
        // Network addresses are mounted once the window is there (it may ask for a password):
        // the first one takes the window's only tab when nothing else was asked for.
        let new_tabs = !locations.is_empty();
        let w = actx.open_window(locations);
        for (i, uri) in remote.iter().enumerate() {
            super::shares::open_remote(&w, uri, new_tabs || i > 0);
        }
        if super::selftest::script_path().is_some() && !super::selftest::active() {
            super::selftest::start(&actx, &w);
        }
        glib::ExitCode::SUCCESS
    });

    let c = ctx.clone();
    app.connect_shutdown(move |_| {
        if let Some(a) = c.borrow().as_ref() {
            a.save_settings_now();
        }
        failbrauwser::archive::Helper::global().shutdown();
    });

    app.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_resolve_relative_paths_and_flags() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("x")).unwrap();
        std::fs::write(dir.path().join("f.txt"), "").unwrap();
        let args: Vec<std::ffi::OsString> = ["failbrauwser", "--daemon", "x", "f.txt"].iter().map(Into::into).collect();
        let (locs, _, daemon, quit) = parse_args(&args, Some(dir.path().to_path_buf()));
        assert!(daemon);
        assert!(!quit);
        assert_eq!(locs, vec![Location::Dir(dir.path().join("x")), Location::Dir(dir.path().to_path_buf())]);
    }
}
