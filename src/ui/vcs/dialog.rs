//! What the Git dialogs share: TortoiseGit's titles ("path - Commit - TortoiseGit"), its
//! group boxes and rows of labelled fields, and finding them again in tests.

use super::super::window::Window;
use gtk::prelude::*;
use std::path::Path;

/// What every TortoiseGit window title ends in.
pub const SUFFIX: &str = "TortoiseGit";

/// "path - What - TortoiseGit".
pub fn title(path: &Path, what: &str) -> String {
    format!("{} - {what} - {SUFFIX}", path.display())
}

/// A dialog window of its own, above the file manager's window, with the icon of what it does.
pub fn new(w: &Window, title: &str, icon: &str) -> gtk::Dialog {
    let d = gtk::Dialog::new();
    d.set_title(title);
    d.set_transient_for(Some(&w.win));
    d.set_destroy_with_parent(true);
    d.set_modal(false);
    d.set_type_hint(gtk::gdk::WindowTypeHint::Dialog);
    if let Some(pb) = super::icons::pixbuf(icon, 32) {
        d.set_icon(Some(&pb));
    }
    let area = d.content_area();
    area.set_border_width(10);
    area.set_spacing(8);
    d
}

/// TortoiseGit's "OK" and "Cancel" (OK answers on Enter).
pub fn ok_cancel(d: &gtk::Dialog) {
    d.add_button("OK", gtk::ResponseType::Ok);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    d.set_default_response(gtk::ResponseType::Ok);
}

/// A group box with a title around `child`.
pub fn frame(label: &str, child: &impl IsA<gtk::Widget>) -> gtk::Frame {
    let f = gtk::Frame::new(Some(label));
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 0);
    inner.set_border_width(8);
    inner.pack_start(child, true, true, 0);
    f.add(&inner);
    f
}

/// A grid of "Label: field" rows.
pub fn grid() -> gtk::Grid {
    let g = gtk::Grid::new();
    g.set_row_spacing(6);
    g.set_column_spacing(8);
    g
}

/// Adds "label  field" as row `row` of `g`.
pub fn grid_row(g: &gtk::Grid, row: i32, label: &str, field: &impl IsA<gtk::Widget>) {
    let l = gtk::Label::new(Some(label));
    l.set_xalign(0.0);
    g.attach(&l, 0, row, 1, 1);
    field.set_hexpand(true);
    g.attach(field, 1, row, 1, 1);
}

pub fn label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l
}

pub fn check(label: &str, name: &str, active: bool) -> gtk::CheckButton {
    let c = gtk::CheckButton::with_label(label);
    c.set_widget_name(name);
    c.set_active(active);
    c
}

pub fn entry(name: &str, text: &str) -> gtk::Entry {
    let e = gtk::Entry::new();
    e.set_widget_name(name);
    e.set_text(text);
    e.set_activates_default(true);
    e
}

/// A drop-down list that can also be typed into, holding `items` (first: `active`).
pub fn combo(name: &str, items: &[String], active: &str) -> gtk::ComboBoxText {
    let c = gtk::ComboBoxText::with_entry();
    c.set_widget_name(name);
    for i in items {
        c.append(Some(i), i);
    }
    if let Some(e) = c.child().and_then(|e| e.downcast::<gtk::Entry>().ok()) {
        e.set_text(active);
        e.set_activates_default(true);
    }
    c
}

/// What a combo box shows (typed or picked).
pub fn combo_text(c: &gtk::ComboBoxText) -> String {
    c.active_text().map(|s| s.to_string()).or_else(|| c.child().and_then(|e| e.downcast::<gtk::Entry>().ok()).map(|e| e.text().to_string())).unwrap_or_default().trim().to_string()
}

/// Radio buttons in a row; the first is chosen.
pub fn radios(names: &[(&str, &str)]) -> Vec<gtk::RadioButton> {
    let mut out: Vec<gtk::RadioButton> = Vec::new();
    for (label, name) in names {
        let r = match out.first() {
            Some(first) => gtk::RadioButton::with_label_from_widget(first, label),
            None => gtk::RadioButton::with_label(label),
        };
        r.set_widget_name(name);
        out.push(r);
    }
    out
}

/// "Base On" as TortoiseGit asks it: HEAD, a branch, a tag or a commit; the version picked.
pub struct BaseOn {
    pub frame: gtk::Frame,
    radios: Vec<gtk::RadioButton>,
    branch: gtk::ComboBoxText,
    tag: gtk::ComboBoxText,
    commit: gtk::Entry,
}

impl BaseOn {
    /// With HEAD offered first when `head` is set; else the branch is chosen first.
    pub fn new(title: &str, branches: &[String], tags: &[String], current: &str, head: bool) -> BaseOn {
        let g = grid();
        let mut names = vec![];
        if head {
            names.push(("HEAD", "base-head"));
        }
        names.extend([("Branch", "base-branch"), ("Tag", "base-tag"), ("Commit", "base-commit")]);
        let radios = radios(&names);
        let branch = combo("base-branch-name", branches, if branches.iter().any(|b| b == current) { current } else { branches.first().map(String::as_str).unwrap_or("") });
        let tag = combo("base-tag-name", tags, tags.last().map(String::as_str).unwrap_or(""));
        let commit = entry("base-commit-id", "");
        let mut row = 0;
        if head {
            g.attach(&radios[0], 0, row, 1, 1);
            row += 1;
        }
        let off = usize::from(head);
        for (i, field) in [branch.upcast_ref::<gtk::Widget>(), tag.upcast_ref(), commit.upcast_ref()].into_iter().enumerate() {
            g.attach(&radios[off + i], 0, row, 1, 1);
            field.set_hexpand(true);
            g.attach(field, 1, row, 1, 1);
            row += 1;
        }
        // Typing into a field picks its choice.
        for (i, field) in [branch.upcast_ref::<gtk::Widget>(), tag.upcast_ref(), commit.upcast_ref()].into_iter().enumerate() {
            let r = radios[off + i].clone();
            let entry = field.downcast_ref::<gtk::Entry>().cloned().or_else(|| field.downcast_ref::<gtk::ComboBoxText>().and_then(|c| c.child()).and_then(|c| c.downcast::<gtk::Entry>().ok()));
            if let Some(e) = entry {
                e.connect_changed(move |e| {
                    if e.has_focus() {
                        r.set_active(true);
                    }
                });
            }
        }
        BaseOn { frame: frame(title, &g), radios, branch, tag, commit }
    }

    /// The version chosen ("HEAD", a branch or tag name, a commit id).
    pub fn rev(&self) -> String {
        let active = self.radios.iter().position(|r| r.is_active()).unwrap_or(0);
        let off = self.radios.len() - 3;
        if active < off {
            return "HEAD".into();
        }
        let v = match active - off {
            0 => combo_text(&self.branch),
            1 => combo_text(&self.tag),
            _ => self.commit.text().trim().to_string(),
        };
        if v.is_empty() { "HEAD".into() } else { v }
    }

    /// Chooses "Commit" with this id.
    pub fn set_commit(&self, id: &str) {
        self.commit.set_text(id);
        if let Some(r) = self.radios.last() {
            r.set_active(true);
        }
    }
}

/// A read-only text area for messages and output, monospaced.
pub fn text_view(name: &str, editable: bool) -> (gtk::ScrolledWindow, gtk::TextView) {
    let tv = gtk::TextView::new();
    tv.set_widget_name(name);
    tv.set_monospace(true);
    tv.set_editable(editable);
    tv.set_cursor_visible(editable);
    tv.set_wrap_mode(gtk::WrapMode::WordChar);
    tv.set_left_margin(4);
    tv.set_right_margin(4);
    let sw = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.add(&tv);
    (sw, tv)
}

pub fn buffer_text(tv: &gtk::TextView) -> String {
    let b = tv.buffer().expect("text buffer");
    b.text(&b.start_iter(), &b.end_iter(), false).map(|s| s.to_string()).unwrap_or_default()
}

/// Shows an error the way TortoiseGit does: a message box titled "TortoiseGit".
pub fn error(parent: &impl IsA<gtk::Window>, text: &str) {
    let d = gtk::MessageDialog::new(Some(parent), gtk::DialogFlags::DESTROY_WITH_PARENT, gtk::MessageType::Error, gtk::ButtonsType::Ok, text);
    d.set_title(SUFFIX);
    d.connect_response(|d, _| d.close());
    d.show();
}

/// Asks a yes/no question in a "TortoiseGit" message box.
pub fn confirm(parent: &impl IsA<gtk::Window>, text: &str, detail: &str, yes: &str, on_yes: impl Fn() + 'static) {
    let d = gtk::MessageDialog::new(Some(parent), gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT, gtk::MessageType::Question, gtk::ButtonsType::None, text);
    d.set_title(SUFFIX);
    if !detail.is_empty() {
        d.set_secondary_text(Some(detail));
    }
    d.add_button(yes, gtk::ResponseType::Yes);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    d.set_default_response(gtk::ResponseType::Yes);
    d.connect_response(move |d, r| {
        unsafe { d.destroy() };
        if r == gtk::ResponseType::Yes {
            on_yes();
        }
    });
    d.show();
}

/// Keeps `v` alive as long as the widget `w` is (until it is destroyed).
pub fn keep_alive<T: 'static>(w: &impl IsA<gtk::Widget>, v: std::rc::Rc<T>) {
    let slot = std::cell::RefCell::new(Some(v));
    w.connect_destroy(move |_| {
        slot.borrow_mut().take();
    });
}

/// The shown window whose title contains `text` (tests).
#[allow(deprecated)]
pub fn find_window(text: &str) -> Option<gtk::Window> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
        .filter(|w| w.is_visible())
        .find(|w| w.title().is_some_and(|t| t.contains(text)))
}

/// Date and time as the logs show them.
pub fn when(unix: i64) -> String {
    gtk::glib::DateTime::from_unix_local(unix).ok().and_then(|d| d.format("%Y-%m-%d %H:%M:%S").ok()).map(|s| s.to_string()).unwrap_or_default()
}
