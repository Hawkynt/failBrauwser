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
//! `dialog-button <dialog text> : <button>`, `quit`. Blank lines and `#` comments are ignored.

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

/// A visible message dialog whose text contains `text`.
#[allow(deprecated)]
fn find_dialog(text: &str) -> Option<gtk::MessageDialog> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::MessageDialog>().ok())
        .filter(|d| d.is_visible())
        .find(|d| d.text().is_some_and(|t| t.contains(text)) || d.secondary_text().is_some_and(|t| t.contains(text)))
}

fn idle(r: &Runner) -> bool {
    !r.app.jobs.is_busy() && r.window.panes().iter().all(|p| !p.is_loading() && !p.refresh_pending())
}

fn run_step(r: &mut Runner, step: &str) -> Result<(), String> {
    let (cmd, arg) = step.split_once(' ').map(|(c, a)| (c, a.trim())).unwrap_or((step, ""));
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
        "window-snapshot" => {
            // The window drawn into a PNG (works on any compositor, unlike `screenshot`).
            let alloc = w.win.allocation();
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
                "keepboth" => Answer::KeepBoth,
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
            wait(r, move |r| Ok(r.window.current_location().display() == want && idle(r)));
        }
        "expect-location" => {
            let loc = w.current_location().display();
            if loc != arg {
                return Err(format!("location is {loc}"));
            }
        }
        "wait-cell" => {
            // wait-cell <column> <row name> = <text>
            let (col, rest) = arg.split_once(' ').ok_or("usage: wait-cell <column> <name> = <text>")?;
            let (name, want) = rest.split_once(" =").ok_or("usage: wait-cell <column> <name> = <text>")?;
            let want = want.trim();
            let column = match col {
                "size" => super::model::COL_SIZE_TEXT,
                "dirsize" => super::model::COL_DIRSIZE_TEXT,
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
        "expect-tree-root" => {
            let sb = super::sidebar::for_window(&w).ok_or("no sidebar")?;
            let root = sb.root_path().map(|p| p.display().to_string()).unwrap_or_default();
            if root != arg {
                return Err(format!("tree root is {root}"));
            }
        }
        "wait-tree-selected" => {
            let want = arg.to_string();
            wait(r, move |r| {
                let sb = super::sidebar::for_window(&r.window).ok_or("no sidebar")?;
                Ok(sb.selected_path().is_some_and(|p| p.display().to_string() == want))
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
