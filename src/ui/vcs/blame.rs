//! TortoiseGitBlame: every line of a file with the commit, author and date that last
//! changed it, the commit named once per block of lines, newer lines shaded warmer; the
//! status bar tells the commit of the line selected.

use super::super::window::Window;
use super::dialog;
use super::Wc;
use failbrauwser::vcs::git::BlameLine;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static SHOWN: RefCell<Vec<(glib::WeakRef<gtk::Window>, Rc<Vec<BlameLine>>)>> = const { RefCell::new(Vec::new()) };
}

/// The shading of a line by its age: the newest commit warm, the oldest white.
pub fn age_color(time: i64, oldest: i64, newest: i64) -> String {
    let f = if newest > oldest { (time - oldest) as f64 / (newest - oldest) as f64 } else { 1.0 };
    let mix = |a: f64, b: f64| (a + (b - a) * f).round() as u8;
    format!("#{:02x}{:02x}{:02x}", mix(255.0, 255.0), mix(255.0, 214.0), mix(255.0, 150.0))
}

pub fn show(w: &Window, wc: &Wc, path: &str) {
    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    win.set_title(&format!("{} - TortoiseGitBlame", wc.root().join(path).display()));
    win.set_transient_for(Some(&w.win));
    win.set_destroy_with_parent(true);
    win.set_default_size(1000, 700);
    if let Some(pb) = super::icons::pixbuf("blame", 32) {
        win.set_icon(Some(&pb));
    }
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::U32]);
    let view = gtk::TreeView::with_model(&store);
    view.set_widget_name("blame");
    for (title, col, mono, right) in [("Commit", 0, true, false), ("Author", 1, false, false), ("Date", 2, false, false), ("Line", 3, true, true), ("", 4, true, false)] {
        let c = gtk::TreeViewColumn::new();
        c.set_title(title);
        c.set_resizable(true);
        let r = gtk::CellRendererText::new();
        if mono {
            r.set_property("family", "monospace");
        }
        if right {
            r.set_xalign(1.0);
        }
        r.set_property("foreground", "#000000");
        TreeViewColumnExt::pack_start(&c, &r, true);
        TreeViewColumnExt::add_attribute(&c, &r, "text", col);
        TreeViewColumnExt::add_attribute(&c, &r, "cell-background", 5);
        if col == 4 {
            c.set_expand(true);
        }
        view.append_column(&c);
    }
    let sw = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    sw.add(&view);
    outer.pack_start(&sw, true, true, 0);
    let status = gtk::Statusbar::new();
    status.set_widget_name("blame-status");
    outer.pack_start(&status, false, false, 0);
    win.add(&outer);
    win.show_all();
    status.push(0, "Blaming…");
    let (wc, p) = (wc.clone(), path.to_string());
    let (store2, status2, win2, view2) = (store.clone(), status.clone(), win.downgrade(), view.clone());
    super::background(move || wc.blame(&p), move |res| {
        let lines = match res {
            Ok(l) => Rc::new(l),
            Err(e) => {
                status2.push(0, &e);
                return;
            }
        };
        let oldest = lines.iter().map(|l| l.time).min().unwrap_or(0);
        let newest = lines.iter().map(|l| l.time).max().unwrap_or(0);
        let mut prev = String::new();
        for (i, l) in lines.iter().enumerate() {
            let first = l.hash != prev;
            prev = l.hash.clone();
            let it = store2.append();
            let (commit, author, date) = if first { (l.hash[..8].to_string(), l.author.clone(), dialog::when(l.time)) } else { Default::default() };
            store2.set(&it, &[(0, &commit), (1, &author), (2, &date), (3, &l.line.to_string()), (4, &l.text.replace('\t', "    ")), (5, &age_color(l.time, oldest, newest)), (6, &(i as u32))]);
        }
        let commits: std::collections::HashSet<&str> = lines.iter().map(|l| l.hash.as_str()).collect();
        status2.push(0, &format!("Lines: {}    Commits: {}", lines.len(), commits.len()));
        let (st, ls) = (status2.clone(), lines.clone());
        view2.selection().connect_changed(move |sel| {
            if let Some((m, it)) = sel.selected() {
                let i: u32 = m.value(&it, 6).get().unwrap_or(0);
                if let Some(l) = ls.get(i as usize) {
                    st.push(0, &format!("{}  {}  {}  {}", &l.hash[..8], l.author, dialog::when(l.time), l.summary));
                }
            }
        });
        if let Some(w) = win2.upgrade() {
            SHOWN.with(|s| {
                let mut s = s.borrow_mut();
                s.retain(|(w, _)| w.upgrade().is_some());
                s.push((w.downgrade(), lines));
            });
        }
    });
}

/// The lines of the blame window whose title contains `title`, as "author: text" (tests).
pub fn describe(title: &str) -> Option<Vec<String>> {
    SHOWN.with(|s| {
        s.borrow().iter().find_map(|(w, lines)| {
            let w = w.upgrade()?;
            (w.is_visible() && w.title().is_some_and(|t| t.contains(title))).then(|| lines.iter().map(|l| format!("{}: {}", l.author, l.text)).collect())
        })
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn newer_lines_are_warmer() {
        assert_eq!(super::age_color(0, 0, 100), "#ffffff");
        assert_eq!(super::age_color(100, 0, 100), "#ffd696");
        assert_eq!(super::age_color(5, 5, 5), "#ffd696");
    }
}
