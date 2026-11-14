// Console windows on Windows come from the subsystem a program file names, not from what it
// writes: a console (CUI) file started by something with no console of its own gets a fresh one,
// which is the black window. This program is linked as a windowed file (`src\main.rs`), so it has
// none to share — and everything console-shaped it starts would allocate its own. `no_window` is
// what keeps that from happening; measured here on 2026-11-12: the archive helper started through
// a windowed parent appeared on the desktop as a console window titled with its own path, and with
// this flag in place it did not.
//
// Nothing here redirects the standard handles. A windowed program that was given no handles still
// has the C runtime's, which a console-less start backs with the console device rather than
// leaving them unusable: a windowed program started with nothing attached printed on both streams
// and stayed alive, with and without such a guard (docs\WINDOWS.md).

/// `CREATE_NO_WINDOW`: start a console program without giving it a console window.
pub fn no_window(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW);
}

/// The flag's value comes from the Windows API; `tests\win_startup.rs` pins it, because a wrong
/// one here brings the black windows back silently.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
