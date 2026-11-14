//! How the program file asks Windows to start it. A console-subsystem file gets a black
//! console window whenever nothing is already attached to it — a double-click, the Start Menu
//! entry, or the single-file launcher — which is what these scenarios keep out of the release.
//! The answers a terminal still gets are pinned by `cli.rs` and by the terminal runs recorded
//! in docs/WINDOWS.md; the launcher file is windowed by `src/portable.rs` and checked against
//! the packaged artifact there.

#![cfg(windows)]

#[path = "support/pe.rs"]
mod pe;

use std::path::Path;

/// Starting the program shows only its own window: the file is linked as a windowed program,
/// so Windows allocates it no console.
#[test]
fn the_built_program_starts_without_a_console_window() {
    let exe = env!("CARGO_BIN_EXE_failbrauwser");
    let found = pe::subsystem(Path::new(exe)).unwrap_or_else(|why| panic!("{exe} is not a readable image: {why}"));
    assert_eq!(found, pe::SUBSYSTEM_GUI, "the program file starts under subsystem {found} ({}), so Windows hands it a console window", name(found));
}

/// Every console program this program starts is created with `CREATE_NO_WINDOW`, so none of them
/// brings up a black window of its own. The flag's value is the Windows API's, and an off-by-one
/// bit in it would quietly start showing windows again.
#[test]
fn console_children_are_started_with_the_no_window_flag() {
    assert_eq!(failbrauwser::win::console::CREATE_NO_WINDOW, 0x0800_0000, "the flag handed to CreateProcess must be CREATE_NO_WINDOW");
}

fn name(subsystem: u16) -> &'static str {
    match subsystem {
        pe::SUBSYSTEM_CONSOLE => "console program",
        pe::SUBSYSTEM_GUI => "windowed program",
        _ => "neither",
    }
}
