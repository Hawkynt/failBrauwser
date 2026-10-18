//! A few layout tweaks on top of the GTK theme: tighter bars and shortcut rows as tall as
//! the rows of the file list. Only sizes and spacing — colors and fonts stay the theme's —
//! and at application priority, so a user's own gtk.css still wins.

use gtk::prelude::*;
use std::cell::Cell;

pub const CSS: &str = r#"
/* Menu bar and tool bar: less air around items. */
menubar > menuitem { min-height: 0; padding: 2px 8px; }
toolbar.primary-toolbar { padding: 2px 4px; }
toolbar.primary-toolbar toolbutton > button { min-height: 0; min-width: 0; padding: 3px 6px; }
toolbar.primary-toolbar entry { min-height: 26px; padding-top: 0; padding-bottom: 0; }

/* Shortcuts: rows as tall as the file list's, icons not dimmed. */
placessidebar row { min-height: 26px; padding: 0; }
placessidebar row > revealer { padding: 0 8px; }
placessidebar row image.sidebar-icon { opacity: 1; }
placessidebar row image.sidebar-icon:dir(ltr) { padding-right: 6px; }
placessidebar row image.sidebar-icon:dir(rtl) { padding-left: 6px; }
placessidebar separator { margin: 2px 0; }

/* Path bar: framed like a text field in the theme's field colors, buttons inside. */
.fb-pathbar { border: 1px solid @borders; border-radius: 5px; background-color: @theme_base_color; padding: 1px; }
.fb-pathbar button { min-height: 0; padding: 2px 8px; border-radius: 3px; }

/* Our own shortcut rows (Drives) look like the places below them. */
list.fb-shortcuts row { min-height: 26px; padding: 0 8px; }
list.fb-shortcuts row image.sidebar-icon:dir(ltr) { padding-right: 6px; }
list.fb-shortcuts row image.sidebar-icon:dir(rtl) { padding-left: 6px; }

/* The drive list: name, mount point in small print and a thin fill bar. */
list.fb-drives row { padding: 3px 8px; }
list.fb-drives row image.sidebar-icon:dir(ltr) { padding-right: 6px; }
list.fb-drives row image.sidebar-icon:dir(rtl) { padding-left: 6px; }
list.fb-drives label.fb-drive-place { font-size: smaller; }
list.fb-drives levelbar trough { min-height: 4px; padding: 0; }
list.fb-drives levelbar block { min-height: 4px; }
separator.fb-drives-line { margin: 4px 8px; }

/* Status bar: one line of text, not a band. */
statusbar { padding: 1px 8px; }
"#;

thread_local! {
    static LOADED: Cell<bool> = const { Cell::new(false) };
}

/// Whether the stylesheet parsed (checked by the UI tests).
pub fn loaded() -> bool {
    LOADED.with(Cell::get)
}

pub fn install() {
    let provider = gtk::CssProvider::new();
    match provider.load_from_data(CSS.as_bytes()) {
        Ok(()) => {
            if let Some(screen) = gtk::gdk::Screen::default() {
                gtk::StyleContext::add_provider_for_screen(&screen, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
                LOADED.with(|l| l.set(true));
            }
        }
        Err(e) => eprintln!("failbrauwser: stylesheet: {e}"),
    }
}
