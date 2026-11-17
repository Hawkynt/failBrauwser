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

/// Keeps the standard handles this program was given from being handed on to what it starts.
///
/// A handle stays inheritable unless told otherwise, and a start that redirects one of its own
/// standard handles still duplicates every inheritable handle of its parent into the child. So each
/// tool started here would hold a second copy of the pipes of whoever started this program: the
/// reader of those pipes sees no end of file until every copy is closed, which is how a run that
/// ended — or a tool that outlived it — leaves the caller waiting with nothing printed (measured on
/// the win32 UI harness on 2026-11-17: the app had printed its answer, the harness that started it
/// read an empty stream, and the test wrapper waited on it for over ten minutes while one started
/// `git` sat unwaited-for). A queue-mode pipe taken over as a child's input is worse: the start
/// itself can fail with `ERROR_BAD_PIPE` (0x800700e8, 232) before the tool runs.
///
/// The handles keep working for this program; only the passing-on stops. What a child is given
/// explicitly (`Stdio::piped()`, `Stdio::null()`, and the streams a client is read through) is
/// untouched, so every read of a tool's answer works as before, and a child started without naming
/// its streams falls back to the console device — a console this windowed program never had anyway.
pub fn keep_our_handles_from_children() {
    use windows::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT, HANDLE_FLAGS};
    use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let Ok(handle) = (unsafe { GetStdHandle(which) }) else {
            continue;
        };
        if handle.is_invalid() {
            continue;
        }
        // Nothing to answer when this fails: the handles are usable either way, and a program cannot
        // undo what the system gave it.
        unsafe {
            let _ = SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0));
        };
    }
}

#[cfg(test)]
mod tests {
    use super::keep_our_handles_from_children;
    use windows::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT};
    use windows::Win32::System::Console::{GetStdHandle, STD_HANDLE, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};

    /// Whether the handle of one of the standard streams is marked inheritable (None when there is
    /// no such handle in this process at all).
    fn inherits(which: STD_HANDLE) -> Option<bool> {
        let handle = unsafe { GetStdHandle(which) }.ok()?;
        if handle.is_invalid() {
            return None;
        }
        let mut flags = 0u32;
        unsafe { GetHandleInformation(handle, &mut flags) }.ok()?;
        Some(flags & HANDLE_FLAG_INHERIT.0 != 0)
    }

    /// What this program was given stays its own: what it starts gets its own streams, so a tool
    /// still running when this program ends cannot keep a caller's captured stream open.
    #[test]
    fn the_standard_handles_stay_this_programs_own() {
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // Windows marks the standard handles inheritable, which is what this undoes; printed so a
            // passing run still says what the starting state was.
            eprintln!("handle {which:?} before this call: {:?}", inherits(which));
        }
        keep_our_handles_from_children();
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            assert_eq!(inherits(which), Some(false), "handle {which:?} is still marked inheritable");
        }
    }
}
