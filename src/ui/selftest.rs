//! Scripted UI self-test. With `FB_SELFTEST=<script>` the first window runs the script's
//! steps through the real actions, dialogs and jobs, then exits with 0 on success.
//! Used by the UI tests (in a headless compositor) and to take the README screenshots.
//!
//! Script lines: `open <path>`, `tab <path>`, `select <name>…`, `action <name> [arg]`,
//! `answer <replace|skip|keepboth|cancel|delete>`, `text <reply for the next text prompt>`,
//! `wait-idle`, `wait-exists <path>`, `wait-missing <path>`, `expect-rows <n>`,
//! `expect-selected <name>`, `expect-location <text>`, `screenshot <file>`, `sleep <ms>`,
//! `size <w> <h>`, `expect-tree-root <path>`, `wait-tree-selected <path>`, `tree-click <path>`, `tree-expand <path>`, `wait-tree-expanded <path>`,
//! `wait-cell <column> <row> = <text>`, `wait-drive <title>`,
//! `wait-row <name>`, `wait-no-row <name>`, `activate` (open the selection),
//! `wait-opened`, `edit-opened <new content>`, `expect-disabled <win.action>`,
//! `crumb-click <label>`, `expect-crumbs <a > b > c>`, `path-edit`, `expect-path-mode <buttons|text>`,
//! `type-location <text>`, `write-file <path> = <content>`, `sidebar-drives`, `expect-sidebar-drives <selected|unselected>`,
//! `expect-shortcuts <a | b | c>`, `expect-shortcut-marked <a>`, `shortcut-click <entry>`,
//! `shortcut-menu <entry> : <item>` (`Show Hidden Entry/<name>` for the submenu),
//! `shortcut-drag <entry> > <before entry>`, `shortcut-drop <folder> > <before entry>`,
//! `window-snapshot <png>`, `wait-error <text>`, `wait-dialog <text>`,
//! `dialog-button <dialog text> : <button>`, `dialog-entry <dialog text> = <text>`, `expect-dialog-entry <dialog text> = <text>`, `wait-no-dialog <text>`, `properties-choose-app <desktop id>`, `clipboard-text <text>`, `batch-set <field> = <value>`, `dialog-set <dialog> : <field> = <value>`, `expect-dialog-label <dialog> : <text>`, `network-add-server`, `expect-batch-preview <a -> b | …>`, `expect-batch-status <text>`, `properties-button <label>`, `wait-properties-text <text>`,
//! `expect-default-app <content type> = <desktop id>`, `fake-phone <name> = <folder>`,
//! `wait-drive-text <title> : <text>`, `wait-no-drive-text <title> : <text>`,
//! `drive-button <title> : <row text> : <button>`, `expect-tabs <a | b>`, `expect-title <text>`,
//! `fake-share <folder>`, `fake-image <file> = <folder>`, `sleep-begin`, `sleep-end`, `expect-menu-item <label>`, `wait-compare-rows <rows>`, `wait-duplicates <n>`, `setenv NAME=value`, `wait-update-checked`, `expect-update-status <text>`, `update-choose <release|source>`, `update-start`, `wait-update-done`, `run <command>`, `wait-file-text <path> = <text>`, `wait-chooser`, `chooser-open <folder>`, `chooser-select <a | b>`, `expect-chooser-rows <a | b>`, `chooser-name <text>`, `chooser-filter <n>`, `chooser-accept`, `chooser-cancel`, `chooser-activate`, `expect-duplicate-group <*kept | …>`, `duplicate-keep <name>`, `duplicate-click <delete|skip|previous>`, `compare-show-identical <on|off>`, `compare-copy <left|right>`, `close-compare`, `expect-no-menu-item <label>`, `fake-job <seconds> <MB>`, `wait-progress-chart <points>`, `expect-progress-text <text>`, `progress-snapshot <png>`,
//! `expect-network-subtitle <row> = <text>`, `wait-side-drive <name> : <place>`, `wait-no-side-drive <name>`, `expect-no-side-drive-at <path>`,
//! `side-drive-click <name> : <place>`, `side-drive-menu <name> : <place> : <item>`,
//! `expect-side-drive-marked <name> : <place>`, `quit`. Blank lines and `#` comments are ignored.

use super::app::AppCtx;
use super::window::Window;
use failbrauwser::location::Location;
use failbrauwser::ops::job::Answer;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

thread_local! {
    /// When the stall watcher last ran, and the longest gap between its runs.
    static STALL: Cell<(Instant, Duration)> = Cell::new((Instant::now(), Duration::ZERO));
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static ANSWER: Cell<Option<Answer>> = const { Cell::new(None) };
    static TEXT: RefCell<VecDeque<String>> = const { RefCell::new(VecDeque::new()) };
    static OPENED: RefCell<Option<std::path::PathBuf>> = const { RefCell::new(None) };
}

/// Instead of launching an application: the file it would have opened.
pub fn record_opened(path: &Path) {
    OPENED.with(|o| *o.borrow_mut() = Some(path.to_path_buf()));
}

pub fn active() -> bool {
    ACTIVE.with(Cell::get)
}

/// The scripted answer for job questions, if the script set one.
pub fn scripted_answer() -> Option<Answer> {
    ANSWER.with(Cell::get)
}

/// The scripted reply for the next text prompt.
pub fn scripted_text() -> Option<String> {
    TEXT.with(|t| t.borrow_mut().pop_front())
}

/// The next scripted reply, only if it is one for this (`f` says so).
pub fn scripted_text_if(f: impl Fn(&str) -> bool) -> Option<String> {
    TEXT.with(|t| {
        let mut t = t.borrow_mut();
        if t.front().is_some_and(|x| f(x)) { t.pop_front() } else { None }
    })
}

pub fn script_path() -> Option<String> {
    std::env::var("FB_SELFTEST").ok().filter(|s| !s.is_empty())
}

struct Runner {
    steps: VecDeque<(usize, String)>,
    window: Rc<Window>,
    app: Rc<AppCtx>,
    waiting: Option<(Instant, Box<dyn Fn(&Runner) -> Result<bool, String>>)>,
    /// The step being run or waited for, for failure messages.
    current: (usize, String),
}

fn fail(line: usize, msg: &str) -> ! {
    eprintln!("SELFTEST FAIL (line {line}): {msg}");
    std::process::exit(1);
}

pub fn start(app: &Rc<AppCtx>, window: &Rc<Window>) {
    let Some(path) = script_path() else { return };
    ACTIVE.with(|a| a.set(true));
    if !super::style::loaded() {
        fail(0, "stylesheet did not load");
    }
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("SELFTEST FAIL: cannot read {path}: {e}");
        std::process::exit(2);
    });
    let steps = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim().to_string()))
        .filter_map(|(i, l)| {
            // A `!win ` line runs on Windows only, a `!unix ` line on Unix only;
            // the other platform never sees the line at all.
            let (marker, mine) = match l.strip_prefix("!win ") {
                Some(_) => ("!win ", cfg!(windows)),
                None => match l.strip_prefix("!unix ") {
                    Some(_) => ("!unix ", cfg!(unix)),
                    None => ("", true),
                },
            };
            mine.then(|| (i, l[marker.len()..].to_string()))
        })
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let runner = Rc::new(RefCell::new(Runner { steps, window: window.clone(), app: app.clone(), waiting: None, current: (0, String::new()) }));
    glib::timeout_add_local(Duration::from_millis(50), move || {
        let mut r = runner.borrow_mut();
        if let Some((since, check)) = r.waiting.take() {
            match check(&r) {
                Ok(true) => {}
                Ok(false) if since.elapsed() < Duration::from_secs(20) => {
                    r.waiting = Some((since, check));
                    return glib::ControlFlow::Continue;
                }
                Ok(false) => {
                    let p = r.window.current_pane();
                    let archive_root: Vec<String> = match p.location() {
                        Location::Archive(a) => failbrauwser::archive::vfs::Vfs::global()
                            .cached(&a)
                            .map(|x| x.tree.children("").unwrap_or_default().iter().map(|n| n.path.clone()).collect())
                            .unwrap_or_default(),
                        _ => Vec::new(),
                    };
                    let selected: Vec<String> = p.selected_items().iter().map(|i| i.display_name()).collect();
                    fail(
                        r.current.0,
                        &format!(
                            "{}: timed out (location {}, {} rows, selected {:?}, archive root {:?}, error {:?}, loading {}, jobs busy {})",
                            r.current.1,
                            p.location().display(),
                            p.item_count(),
                            selected,
                            archive_root,
                            p.error(),
                            p.is_loading(),
                            r.app.jobs.is_busy()
                        ),
                    )
                }
                Err(e) => fail(r.current.0, &format!("{}: {e}", r.current.1)),
            }
        }
        let Some((line, step)) = r.steps.pop_front() else {
            println!("SELFTEST OK");
            std::process::exit(0);
        };
        r.current = (line, step.clone());
        if let Err(e) = run_step(&mut r, &step) {
            fail(line, &format!("{step}: {e}"));
        }
        glib::ControlFlow::Continue
    });
}

/// The first widget of type `T` inside `w`.
fn find_widget<T: IsA<gtk::Widget>>(w: &gtk::Widget) -> Option<T> {
    if let Some(t) = w.downcast_ref::<T>() {
        return Some(t.clone());
    }
    w.downcast_ref::<gtk::Container>()?.children().iter().find_map(find_widget::<T>)
}

/// Sets a field of a dialog: text, number, check box (`on`) or choice (its id).
fn set_field(w: &gtk::Widget, value: &str) -> Result<(), String> {
    if let Some(spin) = w.downcast_ref::<gtk::SpinButton>() {
        spin.set_value(value.parse().map_err(|_| "not a number")?);
    } else if let Some(e) = w.downcast_ref::<gtk::Entry>() {
        e.set_text(value);
    } else if let Some(c) = w.downcast_ref::<gtk::CheckButton>() {
        c.set_active(value == "on");
    } else if let Some(c) = w.downcast_ref::<gtk::ComboBoxText>() {
        c.set_active_id(Some(value));
    } else {
        return Err("cannot set that field".into());
    }
    Ok(())
}

/// The widget under `w` with this name.
fn find_named(w: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
    if w.widget_name() == name {
        return Some(w.clone());
    }
    w.downcast_ref::<gtk::Container>()?.children().iter().find_map(|c| find_named(c, name))
}

/// The Properties dialog shown.
#[allow(deprecated)]
fn properties_dialog() -> Option<gtk::Dialog> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Dialog>().ok())
        .find(|d| d.is_visible() && d.title().is_some_and(|t| t.ends_with("— Properties")))
}

/// A button under `w` whose label (without mnemonic marks) is `label`.
fn find_button(w: &gtk::Widget, label: &str) -> Option<gtk::Button> {
    if let Some(b) = w.downcast_ref::<gtk::Button>() {
        if b.label().is_some_and(|l| l.replace('_', "") == label) {
            return Some(b.clone());
        }
    }
    w.downcast_ref::<gtk::Container>()?.children().iter().find_map(|c| find_button(c, label))
}

/// The texts of the visible labels under `w`.
fn label_texts(w: &gtk::Widget) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(l) = w.downcast_ref::<gtk::Label>() {
        if l.is_visible() {
            out.push(l.text().to_string());
        }
    }
    if let Some(c) = w.downcast_ref::<gtk::Container>() {
        for ch in c.children() {
            out.extend(label_texts(&ch));
        }
    }
    out
}

/// A visible message dialog whose text contains `text`.
#[allow(deprecated)]
fn find_dialog(text: &str) -> Option<gtk::MessageDialog> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::MessageDialog>().ok())
        .filter(|d| d.is_visible())
        .find(|d| d.text().is_some_and(|t| t.contains(text)) || d.secondary_text().is_some_and(|t| t.contains(text)))
}

/// Path-text equality that ignores separator spelling: scenario lines keep `$T/…`
/// slashes on Windows while location displays come back with `\`. On Unix the
/// strings must match exactly (`\` is an ordinary name character there).
fn same_path_text(got: &str, want: &str) -> bool {
    let (got, want) = if cfg!(windows) { (got.replace('/', "\\"), want.replace('/', "\\")) } else { (got.to_string(), want.to_string()) };
    got == want
}

fn idle(r: &Runner) -> bool {
    !r.app.jobs.is_busy() && r.window.panes().iter().all(|p| !p.is_loading() && !p.refresh_pending())
}

/// The title the drives page gives the machine's own system volume: Windows scripts
/// use `$C` for it where Linux scripts name the root filesystem `File System`.
#[cfg(windows)]
fn system_drive_title() -> String {
    let drive = std::env::var("SYSTEMDRIVE").unwrap_or_else(|_| "C:".into());
    failbrauwser::win::storage::volumes().into_iter().find(|v| v.mount_point().is_some_and(|m| m.starts_with(drive.as_str()))).map(|v| v.title()).unwrap_or_default()
}

fn run_step(r: &mut Runner, step: &str) -> Result<(), String> {
    let (cmd, arg) = step.split_once(' ').map(|(c, a)| (c, a.trim())).unwrap_or((step, ""));
    let arg_c = arg.to_string();
    #[cfg(windows)]
    let arg_c = if arg_c.contains("$C") { arg_c.replace("$C", &system_drive_title()) } else { arg_c };
    let arg = arg_c.as_str();
    let w = r.window.clone();
    match cmd {
        "open" => {
            let loc = Location::parse(arg, failbrauwser::archive::is_browsable_name).ok_or("no such location")?;
            w.navigate(loc);
            wait(r, |r| Ok(idle(r)));
        }
        "tab" => {
            let loc = Location::parse(arg, failbrauwser::archive::is_browsable_name).ok_or("no such location")?;
            w.add_tab(loc, true);
            wait(r, |r| Ok(idle(r)));
        }
        "select" => {
            let names: Vec<OsString> = arg.split(" | ").map(OsString::from).collect();
            let names = w.current_pane().keys_for_names(&names);
            w.current_pane().select_names(&names);
            let got = w.current_pane().selected_items().len();
            if got != names.len() {
                return Err(format!("selected {got} of {}", names.len()));
            }
        }
        "action" => {
            let (name, param) = arg.split_once(' ').map(|(n, p)| (n, Some(p))).unwrap_or((arg, None));
            let variant = param.map(|p| p.to_variant());
            let group: Option<gio::ActionGroup> = if let Some(n) = name.strip_prefix("win.") {
                w.win.lookup_action(n).map(|_| w.win.clone().upcast())
            } else if let Some(n) = name.strip_prefix("app.") {
                r.app.app.lookup_action(n).map(|_| r.app.app.clone().upcast())
            } else {
                None
            };
            let group = group.ok_or("unknown action")?;
            let short = name.split_once('.').map(|x| x.1).unwrap_or(name);
            if !group.is_action_enabled(short) {
                return Err("action is disabled".into());
            }
            group.activate_action(short, variant.as_ref());
        }
        "sidebar-drives" => {
            super::sidebar::for_window(&w).ok_or("no sidebar")?.click_drives();
            wait(r, |r| Ok(idle(r)));
        }
        "expect-shortcuts" => {
            let got = super::sidebar::for_window(&w).ok_or("no sidebar")?.labels().join(" | ");
            if got != arg {
                return Err(format!("side panel shows {got:?}"));
            }
        }
        "expect-shortcut-marked" => {
            let got = super::sidebar::for_window(&w).ok_or("no sidebar")?.marked().join(" | ");
            if got != arg {
                return Err(format!("marked: {got:?}"));
            }
        }
        "shortcut-click" => {
            super::sidebar::for_window(&w).ok_or("no sidebar")?.click_label(arg)?;
            wait(r, |r| Ok(idle(r)));
        }
        "shortcut-menu" => {
            // shortcut-menu <entry> : <item>   (entry empty: the empty space)
            let (label, item) = arg.split_once(':').ok_or("need <entry> : <item>")?;
            super::sidebar::for_window(&w).ok_or("no sidebar")?.menu_item(label.trim(), item.trim())?;
        }
        "shortcut-drag" => {
            // shortcut-drag <entry> > <before entry>   (before empty: to the end)
            let (label, before) = arg.split_once('>').ok_or("need <entry> > <before>")?;
            super::sidebar::for_window(&w).ok_or("no sidebar")?.drag_before(label.trim(), before.trim())?;
        }
        "shortcut-drop" => {
            // shortcut-drop <folder> > <before entry>
            let (dir, before) = arg.split_once('>').ok_or("need <folder> > <before>")?;
            super::sidebar::for_window(&w).ok_or("no sidebar")?.drop_dirs_before(vec![PathBuf::from(dir.trim())], before.trim())?;
        }
        "wait-error" => {
            let want = arg.to_string();
            wait(r, move |r| Ok(r.window.current_pane().error().is_some_and(|e| e.contains(&want))));
        }
        "wait-dialog" => {
            let want = arg.to_string();
            wait(r, move |_| Ok(find_dialog(&want).is_some()));
        }
        "dialog-button" => {
            // dialog-button <dialog text> : <button label>
            let (text, button) = arg.split_once(':').ok_or("need <dialog text> : <button>")?;
            let d = find_dialog(text.trim()).ok_or("no such dialog")?;
            fn find(w: &gtk::Widget, label: &str) -> Option<gtk::Button> {
                if let Some(b) = w.downcast_ref::<gtk::Button>() {
                    if b.label().is_some_and(|l| l.replace('_', "") == label) {
                        return Some(b.clone());
                    }
                }
                let c = w.downcast_ref::<gtk::Container>()?;
                c.children().iter().find_map(|ch| find(ch, label))
            }
            find(d.upcast_ref(), button.trim()).ok_or("no such button")?.clicked();
        }
        "dialog-entry" => {
            // dialog-entry <dialog text> = <text>: types into the dialog's text field.
            let (text, value) = arg.split_once(" = ").ok_or("need <dialog text> = <text>")?;
            let d = find_dialog(text.trim()).ok_or("no such dialog")?;
            let entry = find_widget::<gtk::Entry>(d.upcast_ref()).ok_or("the dialog has no text field")?;
            entry.set_text(value);
        }
        "expect-dialog-entry" => {
            let (text, want) = arg.split_once(" = ").ok_or("need <dialog text> = <text>")?;
            let d = find_dialog(text.trim()).ok_or("no such dialog")?;
            let got = find_widget::<gtk::Entry>(d.upcast_ref()).ok_or("the dialog has no text field")?.text();
            if got != want {
                return Err(format!("the field holds {got:?}"));
            }
        }
        "wait-no-dialog" => {
            let want = arg.to_string();
            wait(r, move |_| Ok(find_dialog(&want).is_none()));
        }
        "network-add-server" => super::network::add_server(&w.current_pane())?,
        "dialog-set" => {
            // dialog-set <dialog text> : <field name> = <value>
            let (dialog, rest) = arg.split_once(" : ").ok_or("usage: dialog-set <dialog> : <field> = <value>")?;
            let (name, value) = rest.split_once(" = ").ok_or("usage: dialog-set <dialog> : <field> = <value>")?;
            let d = find_dialog(dialog.trim()).ok_or("no such dialog")?;
            set_field(&find_named(d.upcast_ref(), name.trim()).ok_or("no such field")?, value)?;
        }
        "expect-dialog-label" => {
            // expect-dialog-label <dialog text> : <label text>
            let (dialog, want) = arg.split_once(" : ").ok_or("usage: expect-dialog-label <dialog> : <text>")?;
            let d = find_dialog(dialog.trim()).ok_or("no such dialog")?;
            let texts = label_texts(d.upcast_ref());
            if !texts.iter().any(|t| t == want) {
                return Err(format!("labels: {texts:?}"));
            }
        }
        "batch-set" => {
            // batch-set <widget name> = <value>: a field of the batch rename dialog.
            let (name, value) = arg.split_once(" = ").map(|(n, v)| (n.trim(), v)).ok_or("need <field> = <value>")?;
            let d = super::batchrename::open_dialog().ok_or("no batch rename dialog")?;
            set_field(&find_named(d.upcast_ref(), name).ok_or("no such field")?, value)?;
        }
        "expect-batch-preview" => {
            let got = super::batchrename::preview_rows().join(" | ");
            if got != arg {
                return Err(format!("preview is {got:?}"));
            }
        }
        "expect-batch-status" => {
            let d = super::batchrename::open_dialog().ok_or("no batch rename dialog")?;
            let w = find_named(d.upcast_ref(), "batch-status").ok_or("no status")?;
            let text = w.downcast_ref::<gtk::Label>().map(|l| l.text().to_string()).unwrap_or_default();
            if !text.contains(arg) {
                return Err(format!("status is {text:?}"));
            }
        }
        "clipboard-text" => gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD).set_text(arg),
        "properties-button" => {
            // properties-button <label>: clicks a button of the open Properties dialog.
            let d = properties_dialog().ok_or("no Properties dialog")?;
            let b = find_button(d.upcast_ref(), arg).ok_or("no such button")?;
            if !b.is_sensitive() {
                return Err("the button is disabled".into());
            }
            b.clicked();
        }
        "wait-properties-text" => {
            let want = arg.to_string();
            wait(r, move |_| Ok(properties_dialog().is_some_and(|d| label_texts(d.upcast_ref()).iter().any(|t| t.contains(&want)))));
        }
        "properties-choose-app" => {
            // In the open Properties dialog, picks the application with this desktop id.
            #[allow(deprecated)]
            let chooser = gtk::Window::list_toplevels()
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Dialog>().ok())
                .filter(|d| d.is_visible() && d.title().is_some_and(|t| t.ends_with("Properties")))
                .find_map(|d| find_widget::<gtk::AppChooserButton>(d.upcast_ref()))
                .ok_or("no Properties dialog with an application chooser")?;
            let model = chooser.model().ok_or("no model")?;
            let mut found = None;
            model.foreach(|m, _, it| {
                let app = m.value(it, 0).get::<Option<gio::AppInfo>>().ok().flatten();
                if app.is_some_and(|a| a.id().is_some_and(|id| id == arg)) {
                    found = Some(*it);
                    return true;
                }
                false
            });
            chooser.set_active_iter(Some(&found.ok_or_else(|| format!("{arg} is not offered"))?));
        }
        "expect-default-app" => {
            // expect-default-app <content type> = <desktop id>
            let (ct, want) = arg.split_once('=').ok_or("need <type> = <id>")?;
            let got = gio::AppInfo::default_for_type(ct.trim(), false).and_then(|a| a.id()).map(|s| s.to_string()).unwrap_or_default();
            if got != want.trim() {
                return Err(format!("default for {} is {got:?}", ct.trim()));
            }
        }
        "window-snapshot" => {
            // The window drawn into a PNG (works on any compositor, unlike `screenshot`).
            let alloc = w.win.allocation();
            w.win.size_allocate(&alloc);
            let surface = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, alloc.width(), alloc.height()).map_err(|e| e.to_string())?;
            let cr = gtk::cairo::Context::new(&surface).map_err(|e| e.to_string())?;
            w.win.draw(&cr);
            drop(cr);
            let pb = gtk::gdk::pixbuf_get_from_surface(&surface, 0, 0, alloc.width(), alloc.height()).ok_or("cannot read the drawing")?;
            pb.savev(arg, "png", &[]).map_err(|e| e.to_string())?;
        }
        "expect-sidebar-drives" => {
            let sel = super::sidebar::for_window(&w).ok_or("no sidebar")?.drives_selected();
            if sel != (arg == "selected") {
                return Err(format!("drives entry selected: {sel}"));
            }
        }
        "trash-bar" => {
            if !super::trash::click_bar(&w.current_pane(), arg) {
                return Err(format!("trash bar button {arg:?} is not shown or not clickable"));
            }
        }
        "wait-bin" | "wait-no-bin" => {
            // wait-bin <name> / wait-no-bin <name>: the shell's Recycle Bin holds (or no
            // longer holds) an item that was deleted from `name` — Windows twin of the
            // XDG `wait-exists …/Trash/files/…` checks, which have no such paths here.
            #[cfg(windows)]
            {
                let want = arg.to_string();
                let want2 = want.clone();
                let holds = move || failbrauwser::win::recyclebin::list().is_ok_and(|b| b.iter().any(|i| i.original.file_name().is_some_and(|n| n.to_string_lossy() == want)));
                // `cmd`, never `step`: step still carries the argument ("wait-bin a.txt").
                let want_present = cmd == "wait-bin";
                let found = holds();
                if want_present == found {
                    return Ok(());
                }
                // wait() polls until the check answers true, so it must answer for the
                // state this verb is heading to — present for wait-bin, gone for wait-no-bin.
                wait(r, move |_| {
                    let b = failbrauwser::win::recyclebin::list().unwrap_or_default();
                    Ok(b.iter().any(|i| i.original.file_name().is_some_and(|n| n.to_string_lossy() == want2)) == want_present)
                });
            }
            #[cfg(not(windows))]
            return Err(format!("{step} is Windows-only"));
        }
        "expect-icon-size" => {
            // expect-icon-size <list|icons> <pixels>
            let (view, px) = arg.split_once(' ').ok_or("usage: expect-icon-size <list|icons> <px>")?;
            let p = w.current_pane();
            let got = if view == "list" { p.list_icon_size() } else { p.grid_icon_size() };
            if got.to_string() != px {
                return Err(format!("{view} icons are {got} px"));
            }
        }
        "wait-thumbnails" => {
            // At least this many rows show a thumbnail.
            let n: usize = arg.parse().map_err(|_| "bad number")?;
            wait(r, move |r| Ok(r.window.current_pane().thumbnail_count() >= n));
        }
        "expect-thumbnails" => {
            let n: usize = arg.parse().map_err(|_| "bad number")?;
            let got = w.current_pane().thumbnail_count();
            if got != n {
                return Err(format!("{got} thumbnails"));
            }
        }
        "wait-summary" => {
            // The status bar summary contains this text.
            let want = arg.to_string();
            wait(r, move |r| Ok(super::summary::text(&r.window).contains(&want)));
        }
        "find" => {
            super::searchbar::open(&w);
            super::searchbar::type_text(&w, arg);
            wait(r, |r| Ok(idle(r) && !r.window.current_pane().is_searching()));
        }
        "find-options" => {
            // find-options <deep on|off> <archives on|off>
            let mut it = arg.split_whitespace().map(|x| x == "on");
            super::searchbar::set_options(&w, it.next(), it.next());
            wait(r, |r| Ok(idle(r) && !r.window.current_pane().is_searching()));
        }
        "find-close" => {
            super::searchbar::close(&w);
            wait(r, |r| Ok(idle(r)));
        }
        "click-empty" => w.current_pane().clicked_empty(),
        "drives-click-empty" => {
            if !super::drives::click_empty(&w.current_pane()) {
                return Err("clicked a drive, or no drives page".into());
            }
        }
        "drives-select" => {
            if !super::drives::select(&w.current_pane(), arg) {
                return Err(format!("no drive {arg}"));
            }
        }
        "expect-drive-selected" => {
            // expect-drive-selected <title> | none
            let got = super::drives::selected(&w.current_pane()).unwrap_or_else(|| "none".into());
            if got != arg {
                return Err(format!("selected drive: {got}"));
            }
        }
        "expect-page" => {
            // expect-page drives | list | icons  (what the pane shows)
            let got = w.current_pane().stack.visible_child_name().map(|n| n.to_string()).unwrap_or_default();
            if got != arg {
                return Err(format!("showing {got}"));
            }
        }
        "expect-nothing-selected" => {
            let n = w.current_pane().selected_items().len();
            if n != 0 {
                return Err(format!("{n} items selected"));
            }
        }
        "expect-custom-actions" => {
            // expect-custom-actions <a> | <b>  (in menu order)
            let want: Vec<String> = arg.split(" | ").map(str::to_string).collect();
            let got = super::customactions::available(&w);
            if got != want {
                return Err(format!("actions are {got:?}"));
            }
        }
        "jobs-start-paused" => super::jobs::set_start_paused(arg == "on"),
        "jobs-pause" => r.app.jobs.pause_all(arg != "off"),
        "expect-jobs" => {
            // expect-jobs <running> <waiting>
            let mut it = arg.split_whitespace().filter_map(|x| x.parse::<usize>().ok());
            let want = (it.next().unwrap_or(0), it.next().unwrap_or(0));
            let got = r.app.jobs.counts();
            if got != want {
                return Err(format!("jobs (running, waiting) are {got:?}"));
            }
        }
        "run" => {
            // run <shell command>: started, not waited for.
            std::process::Command::new("sh").arg("-c").arg(arg).spawn().map_err(|e| e.to_string())?;
        }
        "wait-file-text" => {
            // wait-file-text <path> = <text it contains>
            let (path, want) = arg.split_once(" = ").ok_or("usage: wait-file-text <path> = <text>")?;
            let (path, want) = (path.to_string(), want.to_string());
            wait(r, move |_| Ok(std::fs::read_to_string(&path).is_ok_and(|t| t.contains(&want))));
        }
        "wait-chooser" => wait(r, |_| Ok(super::chooser::current().is_some_and(|(_, w)| !w.current_pane().is_loading()))),
        "chooser-open" => {
            let (_, cw) = super::chooser::current().ok_or("no chooser")?;
            cw.navigate(Location::Dir(PathBuf::from(arg)));
            wait(r, |_| Ok(super::chooser::current().is_some_and(|(_, w)| !w.current_pane().is_loading())));
        }
        "chooser-select" => {
            let (_, cw) = super::chooser::current().ok_or("no chooser")?;
            let names: Vec<OsString> = arg.split(" | ").map(OsString::from).collect();
            let p = cw.current_pane();
            let keys = p.keys_for_names(&names);
            p.select_names(&keys);
        }
        "expect-chooser-rows" => {
            let (_, cw) = super::chooser::current().ok_or("no chooser")?;
            let got = cw.current_pane().shown_names().join(" | ");
            if got != arg {
                return Err(format!("chooser shows {got:?}"));
            }
        }
        "chooser-name" => super::chooser::current().ok_or("no chooser")?.0.set_name(arg),
        "chooser-filter" => super::chooser::current().ok_or("no chooser")?.0.set_filter(arg.parse().map_err(|_| "bad number")?),
        "chooser-accept" => super::chooser::current().ok_or("no chooser")?.0.accept_now(),
        "chooser-cancel" => super::chooser::current().ok_or("no chooser")?.0.cancel(),
        "chooser-activate" => {
            let (c, cw) = super::chooser::current().ok_or("no chooser")?;
            c.activated(&cw.current_pane().selected_items());
        }
        "setenv" => {
            // setenv <NAME>=<value>
            let (k, v) = arg.split_once('=').ok_or("usage: setenv NAME=value")?;
            // SAFETY: the self-test runs on the main thread before anything reads it.
            unsafe { std::env::set_var(k, v) };
        }
        "wait-update-checked" => wait(r, |_| Ok(super::update::current().is_some_and(|u| !u.is_checking()))),
        "expect-update-status" => {
            let got = super::update::current().ok_or("no update window")?.status_text();
            if !got.contains(arg) {
                return Err(format!("status is {got:?}"));
            }
        }
        "update-choose" => super::update::current().ok_or("no update window")?.choose(arg == "release"),
        "update-start" => super::update::current().ok_or("no update window")?.run(),
        "wait-update-done" => wait(r, |_| Ok(super::update::current().is_some_and(|u| !u.is_running()))),
        "wait-duplicates" => {
            // wait-duplicates <groups>: the search in the duplicates window is done.
            let n: usize = arg.parse().map_err(|_| "bad number")?;
            wait(r, move |_| Ok(super::duplicates::current().is_some_and(|d| !d.is_busy() && d.group_count() == n)));
        }
        "expect-duplicate-group" => {
            // expect-duplicate-group <*kept | other | …> (in the order found)
            let got = super::duplicates::current().ok_or("no duplicates window")?.shown().join(" | ");
            if got != arg {
                return Err(format!("group shows {got:?}"));
            }
        }
        "duplicate-keep" => super::duplicates::current().ok_or("no duplicates window")?.keep_only(arg)?,
        "duplicate-click" => super::duplicates::current().ok_or("no duplicates window")?.click(arg),
        "wait-compare-rows" => {
            // wait-compare-rows <path state | …>: the compare window's rows, once it is done.
            let want = arg.to_string();
            wait(r, move |_| Ok(super::compare::current().is_some_and(|c| !c.is_busy() && c.row_texts().join(" | ") == want)));
        }
        "compare-show-identical" => super::compare::current().ok_or("no compare window")?.set_show_same(arg == "on"),
        "compare-copy" => super::compare::current().ok_or("no compare window")?.click_copy(arg == "right"),
        "close-compare" => unsafe { super::compare::current().ok_or("no compare window")?.window.destroy() },
        "expect-menu-item" | "expect-no-menu-item" => {
            // The right-click menu of the selection has (or has not) this item.
            fn labels(m: &gio::MenuModel, out: &mut Vec<String>) {
                for i in 0..m.n_items() {
                    if let Some(l) = m.item_attribute_value(i, "label", None).and_then(|v| v.get::<String>()) {
                        out.push(l.replace('_', ""));
                    }
                    for link in ["section", "submenu"] {
                        if let Some(sub) = m.item_link(i, link) {
                            labels(&sub, out);
                        }
                    }
                }
            }
            let p = w.current_pane();
            let model = super::extensions::context_menu_model(&w, &p, &p.selected_items());
            let mut all = Vec::new();
            labels(model.upcast_ref(), &mut all);
            if all.iter().any(|l| l == arg) != (cmd == "expect-menu-item") {
                return Err(format!("menu items: {all:?}"));
            }
        }
        "sleep-begin" => super::shares::sleeping(),
        "sleep-end" => {
            let app = r.app.clone();
            glib::spawn_future_local(async move { super::shares::resumed(&app).await });
        }
        "fake-image" => {
            // fake-image <image file> = <folder>: mounting the image shows the folder.
            let (image, dir) = arg.split_once(" = ").ok_or("usage: fake-image <file> = <folder>")?;
            super::drives::add_fake_image(PathBuf::from(image.trim()), PathBuf::from(dir.trim()));
        }
        "fake-share" => {
            // fake-share <folder>: the folder acts like a network share (no change notices).
            super::pane::add_fake_share(PathBuf::from(arg));
        }
        "fake-job" => {
            // fake-job <seconds> <megabytes>: a copy of 20 files at a steady pace.
            let mut it = arg.split_whitespace().filter_map(|x| x.parse::<f64>().ok());
            let (secs, mb) = (it.next().ok_or("usage: fake-job <seconds> <megabytes>")?, it.next().unwrap_or(100.0));
            r.app.jobs.start(
                w.win.upcast_ref(),
                "Copying 20 items to “backup”",
                move |ctx| {
                    use std::sync::atomic::Ordering;
                    let total = (mb * 1e6) as u64;
                    ctx.bytes_total.store(total, Ordering::Relaxed);
                    ctx.files_total.store(20, Ordering::Relaxed);
                    ctx.set_phase(failbrauwser::ops::job::Phase::Working);
                    let steps = (secs * 20.0).max(1.0) as u64;
                    for i in 1..=steps {
                        ctx.wait_while_paused();
                        if ctx.is_cancelled() {
                            return Err(failbrauwser::ops::fastcopy::cancelled());
                        }
                        // Faster in the middle, like a copy that meets bigger files.
                        let wave = 0.6 + 0.4 * (i as f64 / steps as f64 * std::f64::consts::PI).sin();
                        ctx.add_bytes(((total / steps) as f64 * wave) as u64);
                        let file = (i * 20 / steps).min(19);
                        ctx.files_done.store(file, Ordering::Relaxed);
                        ctx.set_current(format!("holiday-video-{:02}.mkv", file + 1));
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    ctx.bytes_done.store(total, Ordering::Relaxed);
                    Ok(Vec::new())
                },
                |_| {},
            );
        }
        "wait-progress-chart" => {
            // wait-progress-chart <points>: the progress window shows a speed chart this long.
            let n: usize = arg.parse().map_err(|_| "bad number")?;
            wait(r, move |r| Ok(r.app.jobs.progress_window().is_some() && r.app.jobs.chart_points() >= n));
        }
        "expect-progress-text" => {
            fn texts(w: &gtk::Widget, out: &mut Vec<String>) {
                if let Some(l) = w.downcast_ref::<gtk::Label>() {
                    if l.is_visible() {
                        out.push(l.text().to_string());
                    }
                }
                if let Some(c) = w.downcast_ref::<gtk::Container>() {
                    c.children().iter().for_each(|ch| texts(ch, out));
                }
            }
            let win = r.app.jobs.progress_window().ok_or("no progress window")?;
            let mut all = Vec::new();
            texts(win.upcast_ref(), &mut all);
            if !all.iter().any(|t| t.contains(arg)) {
                return Err(format!("the progress window shows {all:?}"));
            }
        }
        "progress-snapshot" => {
            // The progress window drawn into a PNG.
            let win = r.app.jobs.progress_window().ok_or("no progress window")?;
            let alloc = win.allocation();
            win.size_allocate(&alloc);
            let surface = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, alloc.width(), alloc.height()).map_err(|e| e.to_string())?;
            let cr = gtk::cairo::Context::new(&surface).map_err(|e| e.to_string())?;
            win.draw(&cr);
            drop(cr);
            let pb = gtk::gdk::pixbuf_get_from_surface(&surface, 0, 0, alloc.width(), alloc.height()).ok_or("cannot read the drawing")?;
            pb.savev(arg, "png", &[]).map_err(|e| e.to_string())?;
        }
        "wait-no-jobs" => wait(r, |r| Ok(!r.app.jobs.is_busy())),
        "show-compress" => {
            // Opens the real dialog (no scripted reply queued), for screenshots.
            super::archive::compress(&w);
        }
        "crumb-click" => {
            if !w.pathbar.click(arg) {
                return Err(format!("no button {arg:?}, have {:?}", w.pathbar.labels()));
            }
            wait(r, |r| Ok(idle(r)));
        }
        "expect-last-crumb" => {
            let got = w.pathbar.labels();
            if got.last().map(String::as_str) != Some(arg) {
                return Err(format!("buttons are {got:?}"));
            }
            if !w.pathbar.last_visible() {
                return Err("the last button is scrolled out of view".into());
            }
        }
        "expect-crumbs" => {
            // Labels separated by " > ".
            let want: Vec<String> = arg.split(" > ").map(str::to_string).collect();
            let got = w.pathbar.labels();
            if got != want {
                return Err(format!("buttons are {got:?}"));
            }
        }
        "path-edit" => w.pathbar.edit(),
        "expect-path-mode" => {
            let editing = w.pathbar.is_editing();
            if editing != (arg == "text") {
                return Err(format!("path bar is in {} mode", if editing { "text" } else { "button" }));
            }
        }
        "type-location" => {
            // Types into the text field and presses Enter.
            w.pathbar.entry.set_text(arg);
            w.pathbar.entry.activate();
            wait(r, |r| Ok(idle(r)));
        }
        "expect-disabled" => {
            let short = arg.strip_prefix("win.").ok_or("only win.* actions")?;
            if w.win.is_action_enabled(short) {
                return Err("action is enabled".into());
            }
        }
        "answer" => {
            let a = match arg {
                "replace" => Answer::Replace,
                "skip" => Answer::Skip,
                // For a taken name, "rename" chooses another one.
                "keepboth" | "rename" => Answer::KeepBoth,
                "delete" => Answer::DeleteInstead,
                "retry" => Answer::Retry,
                _ => Answer::Cancel,
            };
            ANSWER.with(|x| x.set(Some(a)));
        }
        "text" => TEXT.with(|t| t.borrow_mut().push_back(arg.to_string())),
        "wait-idle" => wait(r, |r| Ok(idle(r))),
        "wait-exists" => {
            let p = arg.to_string();
            wait(r, move |r| Ok(Path::new(&p).exists() && idle(r)));
        }
        "wait-missing" => {
            let p = arg.to_string();
            wait(r, move |r| Ok(!Path::new(&p).exists() && idle(r)));
        }
        "wait-row" | "wait-no-row" => {
            let (name, present) = (arg.to_string(), cmd == "wait-row");
            wait(r, move |r| Ok(r.window.current_pane().has_name(&name) == present && idle(r)));
        }
        "activate" => {
            let p = w.current_pane();
            p.activate(p.selected_items());
            wait(r, |r| Ok(idle(r)));
        }
        "write-file" => {
            // write-file <path> = <content>
            let (path, content) = arg.split_once(" = ").ok_or("usage: write-file <path> = <content>")?;
            std::fs::write(path, content).map_err(|e| e.to_string())?;
        }
        "wait-opened" => wait(r, |_| Ok(OPENED.with(|o| o.borrow().is_some()))),
        "edit-opened" => {
            // Plays an editor: rewrites the file that was opened.
            let path = OPENED.with(|o| o.borrow().clone()).ok_or("nothing opened")?;
            std::fs::write(&path, arg).map_err(|e| e.to_string())?;
        }
        "watch-stalls" => {
            // From now on, the longest time the window did not get to run (frozen).
            STALL.with(|s| s.set((Instant::now(), Duration::ZERO)));
            glib::timeout_add_local(Duration::from_millis(20), || {
                STALL.with(|s| {
                    let (last, max) = s.get();
                    s.set((Instant::now(), max.max(last.elapsed())));
                });
                glib::ControlFlow::Continue
            });
        }
        "wait-count" => {
            // wait-count <n>: the listing (or search) is complete with n rows.
            let n: usize = arg.parse().map_err(|_| "bad number")?;
            wait(r, move |r| {
                let p = r.window.current_pane();
                Ok(!p.is_searching() && p.item_count() == n && idle(r))
            });
        }
        "expect-max-stall" => {
            // expect-max-stall <ms>: the window was never frozen longer than that.
            let limit: u64 = arg.parse().map_err(|_| "bad number")?;
            let max = STALL.with(|s| s.get().1);
            if max > Duration::from_millis(limit) {
                return Err(format!("the window froze for {} ms", max.as_millis()));
            }
            eprintln!("longest freeze: {} ms", max.as_millis());
        }
        "status-button" => {
            // status-button all|files|folders|subfolders: clicks that button of the status bar.
            fn named(w: &gtk::Widget, name: &str) -> Option<gtk::Button> {
                if w.widget_name() == name {
                    return w.downcast_ref::<gtk::Button>().cloned();
                }
                w.downcast_ref::<gtk::Container>()?.children().iter().find_map(|c| named(c, name))
            }
            let b = named(w.win.upcast_ref(), &format!("show-{arg}")).ok_or("no such button")?;
            if !b.is_sensitive() {
                return Err("the button is disabled".into());
            }
            b.clicked();
        }
        "expect-pressed" => {
            let got = super::showfilter::pressed(&w).join(" | ");
            if got != arg {
                return Err(format!("pressed: {got:?}"));
            }
        }
        "wait-shown" => {
            // wait-shown a | b | c: exactly these names are listed (in any order).
            let mut want: Vec<String> = arg.split(" | ").map(str::to_string).filter(|s| !s.is_empty()).collect();
            want.sort();
            wait(r, move |r| Ok(r.window.current_pane().shown_names() == want && idle(r)));
        }
        "expect-rows" => {
            let n: usize = arg.parse().map_err(|_| "bad number")?;
            let got = w.current_pane().item_count();
            if got != n {
                return Err(format!("{got} rows"));
            }
        }
        "expect-selected" => {
            // Waits: new rows arrive through the file monitor a moment after the file exists.
            let want = vec![arg.to_string()];
            wait(r, move |r| Ok(r.window.current_pane().selected_items().iter().map(|i| i.display_name()).collect::<Vec<_>>() == want));
        }
        "wait-location" => {
            // For locations reached asynchronously (after probing a file).
            let want = arg.to_string();
            wait(r, move |r| Ok(same_path_text(&r.window.current_location().display(), &want) && idle(r)));
        }
        "expect-location" => {
            let loc = w.current_location().display();
            if !same_path_text(&loc, arg) {
                return Err(format!("location is {loc}"));
            }
        }
        "wait-cell" => {
            // wait-cell <column> <row name> = <text>
            let (col, rest) = arg.split_once(' ').ok_or("usage: wait-cell <column> <name> = <text>")?;
            let (name, want) = rest.split_once(" =").ok_or("usage: wait-cell <column> <name> = <text>")?;
            let want = want.trim();
            let column = match col {
                "size" => super::model::COL_DIRSIZE_TEXT,
                "type" => super::model::COL_TYPE,
                "modified" => super::model::COL_MTIME_TEXT,
                _ => return Err("unknown column".into()),
            };
            let (name, want) = (name.to_string(), want.to_string());
            wait(r, move |r| Ok(r.window.current_pane().cell_text(&name, column).as_deref() == Some(want.as_str())));
        }
        "drive-properties" => {
            if !super::drives::show_properties_of(&w.current_pane(), arg) {
                return Err("no such drive".into());
            }
        }
        "wait-drive" => {
            // A volume with this title is listed (e.g. "File System" for /).
            let want = arg.to_string();
            wait(r, move |r| Ok(super::drives::shown_volumes(&r.window.current_pane()).iter().any(|v| v.title() == want)));
        }
        "fake-phone" => {
            // fake-phone <name> = <folder>: a phone whose storages are the folder's subfolders.
            let (name, dir) = arg.split_once(" = ").ok_or("usage: fake-phone <name> = <folder>")?;
            super::phones::add_fake(name.trim(), PathBuf::from(dir.trim()));
        }
        "fake-stick" => {
            // fake-stick <label> = <folder> [: <passphrase> [: lvm]]: a USB stick that mounts on
            // the folder, encrypted when a passphrase is given, with an LVM volume inside.
            let (label, rest) = arg.split_once(" = ").ok_or("usage: fake-stick <label> = <folder> [: <passphrase> [: lvm]]")?;
            let (dir, pass) = rest.split_once(" : ").map_or((rest, None), |(d, p)| (d, Some(p.trim().to_string())));
            let (pass, lvm) = match pass.as_deref().and_then(|p| p.strip_suffix(" : lvm")) {
                Some(p) => (Some(p.trim().to_string()), true),
                None => (pass, false),
            };
            super::drives::add_fake(label.trim(), PathBuf::from(dir.trim()), pass, lvm);
        }
        "fake-host" => {
            // fake-host <name> = <share>:<folder>, … [: <password>]: a computer on the network
            // whose shared folders are local folders.
            let (name, rest) = arg.split_once(" = ").ok_or("usage: fake-host <name> = <share>:<folder>, … [: <password>]")?;
            let (list, password) = rest.split_once(" : ").map_or((rest, None), |(l, p)| (l, Some(p.trim().to_string())));
            let shares = list.split(", ").filter_map(|x| x.split_once(':')).map(|(n, d)| (n.trim().to_string(), PathBuf::from(d.trim()))).collect();
            super::network::add_fake(name.trim(), shares, password);
        }
        "wait-network-row" => {
            let want = arg.to_string();
            wait(r, move |r| Ok(super::network::row_titles(&r.window.current_pane()).contains(&want)));
        }
        "expect-network-subtitle" => {
            // expect-network-subtitle <row> = <second line>
            let (row, want) = arg.split_once(" = ").ok_or("usage: expect-network-subtitle <row> = <text>")?;
            let got = super::network::row_subtitle(&w.current_pane(), row.trim()).ok_or("no such row")?;
            if got != want.trim() {
                return Err(format!("second line is {got:?}"));
            }
        }
        "expect-network-rows" => {
            let got = super::network::row_titles(&w.current_pane()).join(" | ");
            if got != arg {
                return Err(format!("rows are {got:?}"));
            }
        }
        "network-open" => super::network::activate(&w.current_pane(), arg)?,
        "network-menu" => {
            // network-menu <row> : <item>
            let (row, item) = arg.split_once(" : ").ok_or("usage: network-menu <row> : <item>")?;
            super::network::menu_item(&w.current_pane(), row.trim(), item.trim())?;
        }
        "color-scheme" => {
            // color-scheme dark|light|default: as if the desktop switched.
            let scheme = match arg {
                "dark" => failbrauwser::appearance::Scheme::Dark,
                "light" => failbrauwser::appearance::Scheme::Light,
                _ => failbrauwser::appearance::Scheme::Default,
            };
            super::appearance::set_scheme(scheme);
        }
        "wait-dark" | "wait-light" => {
            let want = cmd == "wait-dark";
            wait(r, move |r| Ok(super::appearance::window_is_dark(r.window.win.upcast_ref()) == Some(want)));
        }
        "wait-drive-text" => {
            // wait-drive-text <title> : <text>  (a row of that drive shows the text)
            let (title, text) = arg.split_once(" : ").ok_or("usage: wait-drive-text <title> : <text>")?;
            let (title, text) = (title.trim().to_string(), text.trim().to_string());
            wait(r, move |r| Ok(super::drives::row_texts(&r.window.current_pane(), &title).iter().any(|t| t.contains(&text))));
        }
        "wait-no-drive-text" => {
            let (title, text) = arg.split_once(" : ").ok_or("usage: wait-no-drive-text <title> : <text>")?;
            let (title, text) = (title.trim().to_string(), text.trim().to_string());
            wait(r, move |r| Ok(!super::drives::row_texts(&r.window.current_pane(), &title).iter().any(|t| t.contains(&text))));
        }
        "drive-button" => {
            // drive-button <title> : <text in the row> : <button>
            let parts: Vec<&str> = arg.split(" : ").map(str::trim).collect();
            let [title, row, button] = parts[..] else { return Err("usage: drive-button <title> : <row text> : <button>".into()) };
            super::drives::click_button(&w.current_pane(), title, row, button)?;
        }
        "drive-menu" => {
            // drive-menu <title> : <row text> : <item>
            let mut parts = arg.splitn(3, " : ");
            let (Some(title), Some(text), Some(item)) = (parts.next(), parts.next(), parts.next()) else {
                return Err("usage: drive-menu <title> : <row text> : <item>".into());
            };
            super::drives::menu_item(&w.current_pane(), title, text, item)?;
        }
        "expect-tabs" => {
            // expect-tabs <title> | <title>  (the tabs of the window, in order)
            let got = w.panes().iter().map(|p| p.location().title()).collect::<Vec<_>>().join(" | ");
            if got != arg {
                return Err(format!("tabs are {got}"));
            }
        }
        "expect-title" => {
            let got = w.win.title().map(|t| t.to_string()).unwrap_or_default();
            if !got.starts_with(&format!("{arg} - ")) {
                return Err(format!("window title is {got:?}"));
            }
        }
        "wait-side-drive" => {
            // wait-side-drive <name> : <place>  (a row of the side panel's drive list)
            let want = arg.to_string();
            wait(r, move |r| Ok(super::sidebar::for_window(&r.window).ok_or("no sidebar")?.drive_texts().contains(&want)));
        }
        "wait-no-side-drive" => {
            // wait-no-side-drive <name>  (no row of that name)
            let prefix = format!("{arg} : ");
            wait(r, move |r| Ok(!super::sidebar::for_window(&r.window).ok_or("no sidebar")?.drive_texts().iter().any(|t| t.starts_with(&prefix))));
        }
        "expect-no-side-drive-at" => {
            let sb = super::sidebar::for_window(&w).ok_or("no sidebar")?;
            if sb.drive_mounts().iter().any(|m| m == Path::new(arg)) {
                return Err(format!("listed: {:?}", sb.drive_texts()));
            }
        }
        "side-drive-click" => super::sidebar::for_window(&w).ok_or("no sidebar")?.click_drive(arg)?,
        "side-drive-menu" => {
            // side-drive-menu <name> : <place> : <item>
            let (drive, item) = arg.rsplit_once(" : ").ok_or("usage: side-drive-menu <name> : <place> : <item>")?;
            super::sidebar::for_window(&w).ok_or("no sidebar")?.drive_menu_item(drive, item)?;
        }
        "expect-side-drive-marked" => {
            let got = super::sidebar::for_window(&w).ok_or("no sidebar")?.drives_marked().join(" | ");
            if got != arg {
                return Err(format!("marked: {got:?}"));
            }
        }
        "expect-tree-root" => {
            let sb = super::sidebar::for_window(&w).ok_or("no sidebar")?;
            let root = sb.root_path().map(|p| p.display().to_string()).unwrap_or_default();
            if !same_path_text(&root, arg) {
                return Err(format!("tree root is {root}"));
            }
        }
        "wait-tree-selected" => {
            let want = arg.to_string();
            wait(r, move |r| {
                let sb = super::sidebar::for_window(&r.window).ok_or("no sidebar")?;
                Ok(sb.selected_path().is_some_and(|p| same_path_text(&p.display().to_string(), &want)))
            });
        }
        "tree-expand" => {
            let sb = super::sidebar::for_window(&w).ok_or("no sidebar")?;
            if !sb.expand(Path::new(arg)) {
                return Err("folder not in tree".into());
            }
        }
        "wait-tree-expanded" => {
            // Must stay expanded once its subfolders are loaded.
            let dir = std::path::PathBuf::from(arg);
            wait(r, move |r| {
                let sb = super::sidebar::for_window(&r.window).ok_or("no sidebar")?;
                Ok(sb.expanded_with_children(&dir))
            });
        }
        "wait-tree-children" => {
            // wait-tree-children <dir> = <a, b, c>
            let (dir, want) = arg.split_once(" =").ok_or("usage: wait-tree-children <dir> = <names>")?;
            let dir = std::path::PathBuf::from(dir);
            let want: Vec<String> = want.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
            wait(r, move |r| {
                let sb = super::sidebar::for_window(&r.window).ok_or("no sidebar")?;
                Ok(sb.children_of(&dir) == want)
            });
        }
        "tree-menu" => {
            // tree-menu <dir> : <item>
            let (dir, item) = arg.rsplit_once(" : ").ok_or("usage: tree-menu <dir> : <item>")?;
            super::sidebar::for_window(&w).ok_or("no sidebar")?.tree_menu_item(Path::new(dir), item)?;
        }
        "tree-click" => {
            let sb = super::sidebar::for_window(&w).ok_or("no sidebar")?;
            if !sb.click(Path::new(arg)) {
                return Err("folder not in tree".into());
            }
            // The folder opens from an idle callback after the click.
            let want = Location::Dir(PathBuf::from(arg));
            wait(r, move |r| Ok(r.window.current_location() == want && idle(r)));
        }
        "screenshot" => {
            let file = arg.to_string();
            // Let GTK paint first.
            let status = std::process::Command::new("grim").arg(&file).status().map_err(|e| e.to_string())?;
            if !status.success() {
                return Err("grim failed".into());
            }
        }
        "sleep" => {
            let ms: u64 = arg.parse().map_err(|_| "bad number")?;
            let until = Instant::now() + Duration::from_millis(ms);
            wait(r, move |_| Ok(Instant::now() >= until));
        }
        "size" => {
            let mut it = arg.split_whitespace().filter_map(|x| x.parse::<i32>().ok());
            let (Some(x), Some(y)) = (it.next(), it.next()) else { return Err("need width height".into()) };
            w.win.resize(x, y);
        }
        "quit" => {
            println!("SELFTEST OK");
            std::process::exit(0);
        }
        _ => return Err("unknown command".into()),
    }
    Ok(())
}

fn wait(r: &mut Runner, check: impl Fn(&Runner) -> Result<bool, String> + 'static) {
    r.waiting = Some((Instant::now(), Box::new(check)));
}
