//! Self-test steps for the Git integration (see `ui/selftest.rs`):
//!
//! `git-menu <TortoiseGit/Show log>` picks an item of the right-click menu of the selection
//! (or folder), `expect-git-menu <path>` / `expect-no-git-menu <path>`,
//! `wait-git-overlay <name> = <Status|none>`, `wait-git-window <title>`,
//! `wait-no-git-window <title>`, `git-set <title> : <field> = <value>`,
//! `git-click <title> : <button>`, `expect-git-disabled <title> : <button>`,
//! `expect-git-dialog-field <title> : <field> = <text>`, `wait-git-rows <title> : <list> = <+a Modified | …>`,
//! `git-tick <title> : <list> : <path> = <on|off>`, `git-activate <title> : <list> : <path>`,
//! `wait-git-result <title> = <text>`, `expect-git-text <title> : <text>`,
//! `wait-git-text <title> : <text>`, `git-close <title>`, `wait-git-log <title> = <a | b>`,
//! `git-log-select <title> : <subject>`, `git-log-menu <title> : <item>`,
//! `wait-git-diff <title> = <= a | ~ b|B>`, `wait-git-blame <title> = <Ann: a | …>`,
//! `git-snapshot <title> = <png>`, `git-open-submenu <item>` (of the open right-click menu), `wait-git-idle`, `wait-git-sync <title>`.

use super::super::window::Window;
use super::{dialog, filelist, icons};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::rc::Rc;

pub type Check = Box<dyn Fn() -> Result<bool, String>>;

/// Runs a Git step: None when `cmd` is not one; Some(check) when the script waits for it.
pub fn step(w: &Rc<Window>, cmd: &str, arg: &str) -> Option<Result<Option<Check>, String>> {
    let r = match cmd {
        "git-menu" => menu_activate(w, arg).map(|_| None),
        "expect-git-menu" | "expect-no-git-menu" => {
            let found = menu_find(w, arg).is_ok();
            if found == (cmd == "expect-git-menu") { Ok(None) } else { Err(format!("menu: {:?}", menu_labels(w))) }
        }
        "wait-git-overlay" => split(arg, " = ").map(|(name, want)| {
            let (ww, name, want) = (Rc::downgrade(w), name.to_string(), want.to_string());
            Some(Box::new(move || {
                let w = ww.upgrade().ok_or("window gone")?;
                let p = w.current_pane();
                let got = p.git_status(&name).and_then(icons::status_from_code).map(|s| s.label().to_string()).unwrap_or_else(|| "none".into());
                seen(&format!("{name}: {got}"));
                Ok(got == want && super::overlay::idle(&p))
            }) as Check)
        }),
        "wait-git-window" => {
            let t = arg.to_string();
            Ok(Some(Box::new(move || Ok(dialog::find_window(&t).is_some())) as Check))
        }
        "wait-no-git-window" => {
            let t = arg.to_string();
            Ok(Some(Box::new(move || Ok(dialog::find_window(&t).is_none())) as Check))
        }
        "git-set" => (|| {
            let (title, rest) = split(arg, " : ")?;
            let (name, value) = split(rest, " = ")?;
            let win = dialog::find_window(title).ok_or("no such window")?;
            set(&named(win.upcast_ref(), name).ok_or_else(|| format!("no field {name}"))?, value)?;
            Ok(None)
        })(),
        "git-click" => (|| {
            let (title, label) = split(arg, " : ")?;
            let win = dialog::find_window(title).ok_or("no such window")?;
            let b = button(win.upcast_ref(), label).ok_or_else(|| format!("no button {label:?}"))?;
            if !b.is_sensitive() {
                return Err(format!("{label} is disabled"));
            }
            match b.downcast_ref::<gtk::LinkButton>() {
                Some(l) => {
                    l.emit_by_name::<bool>("activate-link", &[]);
                }
                None => b.clicked(),
            }
            Ok(None)
        })(),
        "git-open-submenu" => (|| {
            let (menu, _) = w.open_context_menu().ok_or("no right-click menu open")?;
            let item = menu
                .children()
                .into_iter()
                .filter_map(|c| c.downcast::<gtk::MenuItem>().ok())
                .find(|m| {
                    // Model menu items keep their text in a property once they show an icon.
                    let text = if m.find_property("text").is_some() { m.property::<Option<String>>("text") } else { m.label().map(|l| l.to_string()) };
                    text.is_some_and(|l| l.replace('_', "") == arg)
                })
                .ok_or_else(|| format!("no item {arg}"))?;
            menu.select_item(&item);
            Ok(None)
        })(),
        "expect-git-dialog-field" => (|| {
            let (title, rest) = split(arg, " : ")?;
            let (name, want) = split(rest, " = ")?;
            let win = dialog::find_window(title).ok_or("no such window")?;
            let field = named(win.upcast_ref(), name).ok_or_else(|| format!("no field {name}"))?;
            let got = match (field.downcast_ref::<gtk::ComboBoxText>(), field.downcast_ref::<gtk::Entry>()) {
                (Some(c), _) => dialog::combo_text(c),
                (None, Some(e)) => e.text().to_string(),
                _ => return Err(format!("{name} is no field")),
            };
            if got == want { Ok(None) } else { Err(format!("{name} holds {got:?}")) }
        })(),
        "expect-git-disabled" => (|| {
            let (title, label) = split(arg, " : ")?;
            let win = dialog::find_window(title).ok_or("no such window")?;
            let b = button(win.upcast_ref(), label).ok_or_else(|| format!("no button {label:?}"))?;
            if b.is_sensitive() { Err(format!("{label} is enabled")) } else { Ok(None) }
        })(),
        "wait-git-rows" => (|| {
            let (title, rest) = split(arg, " : ")?;
            let (list, want) = split(rest, " = ")?;
            let (t, l, want) = (title.to_string(), list.to_string(), want.to_string());
            Ok(Some(Box::new(move || {
                let Some(fl) = filelist::find(&t, &l) else { return Ok(false) };
                let got = fl.describe().join(" | ");
                seen(&got);
                Ok(got == want)
            }) as Check))
        })(),
        "git-tick" => (|| {
            let (title, rest) = split(arg, " : ")?;
            let (list, rest) = split(rest, " : ")?;
            let (path, v) = split(rest, " = ")?;
            let fl = filelist::find(title, list).ok_or("no such list")?;
            if fl.set_checked(path, v == "on") { Ok(None) } else { Err(format!("no row {path}: {:?}", fl.describe())) }
        })(),
        "git-activate" => (|| {
            let (title, rest) = split(arg, " : ")?;
            let (list, path) = split(rest, " : ")?;
            let fl = filelist::find(title, list).ok_or("no such list")?;
            if fl.activate(path) { Ok(None) } else { Err(format!("no row {path}")) }
        })(),
        "wait-git-result" => split(arg, " = ").map(|(title, want)| {
            let (t, want) = (title.to_string(), want.to_string());
            Some(Box::new(move || {
                let Some(win) = dialog::find_window(&t) else { return Ok(false) };
                let Some(l) = named(win.upcast_ref(), "git-result").and_then(|l| l.downcast::<gtk::Label>().ok()) else { return Ok(false) };
                let text = l.text().to_string();
                if text.is_empty() {
                    return Ok(false);
                }
                if text.contains(&want) {
                    Ok(true)
                } else {
                    let out = named(win.upcast_ref(), "git-output").and_then(|v| v.downcast::<gtk::TextView>().ok()).map(|v| dialog::buffer_text(&v)).unwrap_or_default();
                    Err(format!("result {text:?}; output:\n{out}"))
                }
            }) as Check)
        }),
        "expect-git-text" => (|| {
            let (title, want) = split(arg, " : ")?;
            let win = dialog::find_window(title).ok_or("no such window")?;
            let texts = texts(win.upcast_ref());
            if texts.iter().any(|t| t.contains(want)) { Ok(None) } else { Err(format!("texts: {texts:?}")) }
        })(),
        "wait-git-text" => split(arg, " : ").map(|(title, want)| {
            let (t, want) = (title.to_string(), want.to_string());
            Some(Box::new(move || {
                let Some(w) = dialog::find_window(&t) else { return Ok(false) };
                let all = texts(w.upcast_ref());
                seen(&all.iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" ¦ "));
                Ok(all.iter().any(|x| x.contains(&want)))
            }) as Check)
        }),
        "git-close" => {
            let win = dialog::find_window(arg);
            match win {
                Some(w) => {
                    unsafe { w.destroy() };
                    Ok(None)
                }
                None => Err("no such window".into()),
            }
        }
        "wait-git-log" => split(arg, " = ").map(|(title, want)| {
            let (t, want) = (title.to_string(), want.to_string());
            Some(Box::new(move || {
                let got = super::log::rows(&t).map(|r| r.join(" | ")).unwrap_or_default();
                seen(&got);
                Ok(got == want)
            }) as Check)
        }),
        "git-log-select" => split(arg, " : ").and_then(|(t, s)| if super::log::select(t, s) { Ok(None) } else { Err(format!("no commit {s:?}: {:?}", super::log::rows(t))) }),
        "git-log-menu" => split(arg, " : ").and_then(|(t, item)| super::log::menu_item(w, t, item).map(|_| None)),
        "wait-git-diff" => split(arg, " = ").map(|(title, want)| {
            let (t, want) = (title.to_string(), want.to_string());
            Some(Box::new(move || {
                let got = super::diff::describe(&t).map(|r| r.join(" | ")).unwrap_or_default();
                seen(&got);
                Ok(got == want)
            }) as Check)
        }),
        "wait-git-blame" => split(arg, " = ").map(|(title, want)| {
            let (t, want) = (title.to_string(), want.to_string());
            Some(Box::new(move || {
                let got = super::blame::describe(&t).map(|r| r.join(" | ")).unwrap_or_default();
                seen(&got);
                Ok(got == want)
            }) as Check)
        }),
        "git-snapshot" => split(arg, " = ").and_then(|(title, png)| {
            let win = dialog::find_window(title).ok_or("no such window")?;
            snapshot(&win, png).map(|_| None)
        }),
        "wait-git-idle" => {
            let ww = Rc::downgrade(w);
            Ok(Some(Box::new(move || Ok(ww.upgrade().is_some_and(|w| w.panes().iter().all(super::overlay::idle)))) as Check))
        }
        "wait-git-sync" => {
            let t = arg.to_string();
            Ok(Some(Box::new(move || Ok(!super::sync::busy(&t))) as Check))
        }
        _ => return None,
    };
    Some(r)
}

/// Tells what a waiting step sees, each time it changes (the failure message names the step).
fn seen(what: &str) {
    thread_local! {
        static LAST: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    }
    LAST.with(|l| {
        if *l.borrow() != what {
            eprintln!("  git step sees: {what}");
            *l.borrow_mut() = what.to_string();
        }
    });
}

fn split<'a>(s: &'a str, sep: &str) -> Result<(&'a str, &'a str), String> {
    if let Some(x) = s.split_once(sep) {
        return Ok((x.0.trim(), x.1.trim()));
    }
    // "field =" at the end of a line: an empty value (the line was trimmed).
    match s.strip_suffix(sep.trim_end()) {
        Some(a) if sep.trim_end() != sep => Ok((a.trim(), "")),
        _ => Err(format!("expected “{sep}” in {s:?}")),
    }
}

/// The Git part of the menu for the current selection.
fn model(w: &Window) -> Option<gio::Menu> {
    let p = w.current_pane();
    super::menu_section(w, &p, &p.selected_items())
}

/// Every item as "A/B" (sections flattened), with its action and target.
fn items(m: &gio::MenuModel, prefix: &str, out: &mut Vec<(String, Option<String>, Option<glib::Variant>)>) {
    for i in 0..m.n_items() {
        let label = m.item_attribute_value(i, "label", None).and_then(|v| v.get::<String>()).map(|l| l.replace("__", "\u{1}").replace('_', "").replace('\u{1}', "_"));
        if let Some(sec) = m.item_link(i, "section") {
            items(&sec, prefix, out);
            continue;
        }
        let Some(label) = label else { continue };
        let path = format!("{prefix}{label}");
        let action = m.item_attribute_value(i, "action", None).and_then(|v| v.get::<String>());
        let target = m.item_attribute_value(i, "target", None);
        out.push((path.clone(), action, target));
        if let Some(sub) = m.item_link(i, "submenu") {
            items(&sub, &format!("{path}/"), out);
        }
    }
}

fn menu_labels(w: &Window) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(m) = model(w) {
        items(m.upcast_ref(), "", &mut out);
    }
    out.into_iter().map(|x| x.0).collect()
}

fn menu_find(w: &Window, path: &str) -> Result<(Option<String>, Option<glib::Variant>), String> {
    let m = model(w).ok_or("no Git menu here")?;
    let mut all = Vec::new();
    items(m.upcast_ref(), "", &mut all);
    all.into_iter().find(|x| x.0 == path).map(|x| (x.1, x.2)).ok_or_else(|| format!("no item {path:?} in {:?}", menu_labels(w)))
}

fn menu_activate(w: &Window, path: &str) -> Result<(), String> {
    let (action, target) = menu_find(w, path)?;
    let action = action.ok_or("a submenu")?;
    let name = action.strip_prefix("win.").ok_or("not a window action")?;
    if w.win.lookup_action(name).is_none() {
        return Err(format!("no action {name}"));
    }
    w.win.activate_action(name, target.as_ref());
    Ok(())
}

fn named(w: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
    if w.widget_name() == name {
        return Some(w.clone());
    }
    w.downcast_ref::<gtk::Container>()?.children().iter().find_map(|c| named(c, name))
}

/// The text a button shows: its label, or that of a label inside it.
fn button_text(b: &gtk::Button) -> Option<String> {
    if let Some(l) = b.label() {
        return Some(l.replace('_', ""));
    }
    fn inner(w: &gtk::Widget) -> Option<String> {
        if let Some(l) = w.downcast_ref::<gtk::Label>() {
            return Some(l.text().to_string());
        }
        w.downcast_ref::<gtk::Container>()?.children().iter().find_map(inner)
    }
    inner(b.upcast_ref())
}

fn button(w: &gtk::Widget, label: &str) -> Option<gtk::Button> {
    if let Some(b) = w.downcast_ref::<gtk::Button>() {
        if !b.is::<gtk::CheckButton>() && b.is_visible() && button_text(b).is_some_and(|t| t == label) {
            return Some(b.clone());
        }
    }
    w.downcast_ref::<gtk::Container>()?.children().iter().find_map(|c| button(c, label))
}

fn set(w: &gtk::Widget, value: &str) -> Result<(), String> {
    if let Some(s) = w.downcast_ref::<gtk::SpinButton>() {
        s.set_value(value.parse().map_err(|_| "not a number")?);
    } else if let Some(e) = w.downcast_ref::<gtk::Entry>() {
        e.set_text(value);
    } else if let Some(c) = w.downcast_ref::<gtk::ToggleButton>() {
        c.set_active(value == "on");
    } else if let Some(c) = w.downcast_ref::<gtk::ComboBoxText>() {
        match c.child().and_then(|e| e.downcast::<gtk::Entry>().ok()) {
            Some(e) => e.set_text(value),
            None => {
                if !c.set_active_id(Some(value)) {
                    return Err(format!("no choice {value}"));
                }
            }
        }
    } else if let Some(t) = w.downcast_ref::<gtk::TextView>() {
        t.buffer().ok_or("no buffer")?.set_text(&value.replace("\\n", "\n"));
    } else {
        return Err("cannot set that field".into());
    }
    Ok(())
}

/// Every text shown by labels and text areas under `w`.
fn texts(w: &gtk::Widget) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(l) = w.downcast_ref::<gtk::Label>() {
        out.push(l.text().to_string());
    }
    if let Some(t) = w.downcast_ref::<gtk::TextView>() {
        out.push(dialog::buffer_text(t));
    }
    if let Some(c) = w.downcast_ref::<gtk::Container>() {
        for ch in c.children() {
            out.extend(texts(&ch));
        }
    }
    out
}

/// The window drawn into a PNG.
fn snapshot(win: &gtk::Window, png: &str) -> Result<(), String> {
    let alloc = win.allocation();
    let surface = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, alloc.width(), alloc.height()).map_err(|e| e.to_string())?;
    let cr = gtk::cairo::Context::new(&surface).map_err(|e| e.to_string())?;
    win.draw(&cr);
    drop(cr);
    let pb = gtk::gdk::pixbuf_get_from_surface(&surface, 0, 0, alloc.width(), alloc.height()).ok_or("cannot read the drawing")?;
    pb.savev(png, "png", &[]).map_err(|e| e.to_string())
}
