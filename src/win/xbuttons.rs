// The mouse's back and forward keys cannot be told apart through GDK on Windows. Measured here on
// 2026-11-13 with the numbers winuser.h gives them: a WM_XBUTTONDOWN posted to a GTK toplevel does
// arrive as a GDK button press, but as **button 5 whichever of the two keys it was** — XBUTTON1 and
// XBUTTON2 alike. GDK's win32 backend never reads the high word of the wParam, where Windows keeps
// the two apart, and GDK's button 5 is its wheel-down button: so one of the panel keys reached a
// pane as a wheel click and the other as the same wheel click, which is what "they don't work"
// looked like from the outside. The pane's `8 => back, 9 => forward` mapping in src\ui\pane.rs is
// the X11 numbering, which win32 GDK never produces either.
//
// Both keys and no phantom wheel click come from reading the messages where Windows keeps the
// distinction: a window procedure chained in front of GDK's own takes the press, hands the key to
// the window's back/forward action, and does not pass the X-button messages on — GDK must not see
// them, or it would answer a scroll button for every one. On X11 nothing here runs, and the pane
// keeps seeing buttons 8 and 9 through GDK as before.

use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GWLP_WNDPROC, GetPropW, GetWindowLongPtrW, PostMessageW, RemovePropW, SetPropW,
    SetWindowLongPtrW,
};
use gtk::glib::object::ObjectType;
use gtk::prelude::{ActionGroupExt, ObjectExt, WidgetExt};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, SetLastError, WIN32_ERROR, WPARAM};
use windows::core::PCWSTR;

/// Which of the two extra mouse keys a message names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideKey {
    Back,
    Forward,
}

// The message and button numbers are the Windows API's own. They are spelled out here, and the
// scenarios at the bottom of this file pin them: a wrong digit loses the keys silently, which is
// the same trap `win::console::CREATE_NO_WINDOW` guards against for the black console window.
pub const WM_XBUTTONDOWN: u32 = 0x020B;
pub const WM_XBUTTONUP: u32 = 0x020C;
pub const WM_XBUTTONDBLCLK: u32 = 0x020D;
/// The back key, in the high-order word of an X-button message's wParam.
pub const XBUTTON1: usize = 1;
/// The forward key, likewise.
pub const XBUTTON2: usize = 2;
const WM_DESTROY: u32 = 0x0002;
/// Where the state hangs on the watched window; see `Watch`.
const PROPERTY: &str = "failbrauwser-xbuttons";

/// The key an X-button message's wParam names: the button sits in its high-order word, and a
/// message naming anything else — or nothing — moves nothing.
pub fn side_key(wparam: usize) -> Option<SideKey> {
    match (wparam >> 16) & 0xFFFF {
        XBUTTON1 => Some(SideKey::Back),
        XBUTTON2 => Some(SideKey::Forward),
        _ => None,
    }
}

/// What this window procedure chains to and which window to hand the key to. A window procedure
/// has no argument of its own for user data, so it hangs on the window under `PROPERTY`: that
/// way it lives and dies with the window it belongs to.
struct Watch {
    previous: isize,
    window: gtk::glib::WeakRef<gtk::ApplicationWindow>,
}

// `gdk/gdkwin32.h`: the window handle a GDK toplevel is drawn on. GDK's win32-only headers are not
// bound by the gtk crate, so this is declared by hand; the symbol is in the import lib the build
// already links (`winpack\sysroot\ucrt64\lib\gdk-3.lib`), named here to make that clear.
#[link(name = "gdk-3")]
unsafe extern "system" {
    fn gdk_win32_window_get_handle(window: *mut std::ffi::c_void) -> HWND;
}

/// The handle this toplevel's messages arrive on, once it is realized.
fn handle(window: &gtk::ApplicationWindow) -> Option<HWND> {
    let surface = window.window()?;
    let hwnd = unsafe { gdk_win32_window_get_handle(surface.as_ptr().cast()) };
    (!hwnd.0.is_null()).then_some(hwnd)
}

/// Watches one window's extra mouse keys, once its handle exists: shown, in other words.
pub fn watch(window: &gtk::ApplicationWindow) {
    let Some(hwnd) = handle(window) else {
        return;
    };
    let name = name();
    unsafe {
        // A window built twice, or watched twice, is watched once.
        if !GetPropW(hwnd, name).0.is_null() {
            return;
        }
        let state = Box::into_raw(Box::new(Watch { previous: GetWindowLongPtrW(hwnd, GWLP_WNDPROC), window: window.downgrade() }));
        if SetPropW(hwnd, name, Some(HANDLE(state as *mut std::ffi::c_void))).is_err() {
            drop(Box::from_raw(state));
            return;
        }
        SetWindowLongPtrW(hwnd, GWLP_WNDPROC, side_keys as *const unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT as isize);
    }
}

/// Posts the press-and-release pair one of the mouse's extra keys produces, to this window's own
/// handle and with the same wParam the input driver sends. A scripted scenario uses this in place
/// of a hand on the mouse; the messages enter the queue exactly as a real click's do.
pub fn press(window: &gtk::ApplicationWindow, key: SideKey) -> Result<(), String> {
    let hwnd = handle(window).ok_or("the window has no handle yet")?;
    let which = match key {
        SideKey::Back => XBUTTON1,
        SideKey::Forward => XBUTTON2,
    };
    let name = name();
    // The high word names the key, the low word the ordinary buttons held down: none.
    let wparam = WPARAM(which << 16);
    unsafe {
        // The name is read once here and the pair is posted whole; watching this window is what
        // makes the press mean something.
        if GetPropW(hwnd, name).0.is_null() {
            return Err("this window's extra mouse keys are not being watched".into());
        }
        // The last error is cleared before each post: PostMessageW only sets one on failure, and the
        // windows crate reports whatever it finds sitting there — measured here, that was an
        // unrelated pipe error from a few calls earlier, which read as a plain lie about this call.
        SetLastError(WIN32_ERROR(0));
        if let Err(e) = PostMessageW(Some(hwnd), WM_XBUTTONDOWN, wparam, LPARAM(0)) {
            return Err(format!("the press did not reach {hwnd:?}: {e}"));
        }
        SetLastError(WIN32_ERROR(0));
        if let Err(e) = PostMessageW(Some(hwnd), WM_XBUTTONUP, wparam, LPARAM(0)) {
            return Err(format!("the release did not reach {hwnd:?}: {e}"));
        }
        Ok(())
    }
}

unsafe extern "system" fn side_keys(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let state = GetPropW(hwnd, name()).0 as *mut Watch;
        let Some(watch) = state.as_ref() else {
            // Only reachable once the state is gone, which the WM_DESTROY branch below prevents by
            // un-chaining this procedure before the window's last message. The default procedure is
            // the least destructive answer to a message nobody can act on.
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        let previous = previous(watch.previous);
        match msg {
            WM_XBUTTONDOWN => {
                if let Some(key) = side_key(wparam.0) {
                    if let Some(window) = watch.window.upgrade() {
                        window.activate_action(match key { SideKey::Back => "back", SideKey::Forward => "forward" }, None::<&gtk::glib::Variant>);
                    }
                }
                LRESULT(1)
            }
            // The other half of a press already taken, and a double-press of it: GDK must not see
            // these, or a GDK that ever learns the buttons would move twice per click.
            WM_XBUTTONUP | WM_XBUTTONDBLCLK => LRESULT(1),
            WM_DESTROY => {
                // Back to the procedure that was here, and the state along with the window.
                let prior = watch.previous;
                SetWindowLongPtrW(hwnd, GWLP_WNDPROC, prior);
                let _ = RemovePropW(hwnd, name());
                drop(Box::from_raw(state));
                CallWindowProcW(previous, hwnd, msg, wparam, lparam)
            }
            _ => CallWindowProcW(previous, hwnd, msg, wparam, lparam),
        }
    }
}

/// The procedure this one chains to. A window always has one, but a zero there would be a jump
/// nobody survives, so `None` is what the default procedure is for.
fn previous(raw: isize) -> Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT> {
    (raw != 0).then(|| unsafe { std::mem::transmute::<isize, unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>(raw) })
}

/// The property name, as a wide string that outlives every call: this runs on every message the
/// window takes, so it must not build one per message.
fn name() -> PCWSTR {
    static NAME: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    PCWSTR(NAME.get_or_init(|| PROPERTY.encode_utf16().chain(std::iter::once(0)).collect()).as_ptr())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_back_key_and_the_forward_key_are_read_out_of_the_message() {
        assert_eq!(side_key(XBUTTON1 << 16), Some(SideKey::Back));
        assert_eq!(side_key(XBUTTON2 << 16), Some(SideKey::Forward));
    }

    #[test]
    fn the_ordinary_buttons_and_a_key_named_nowhere_move_nothing() {
        // A message whose high word names no extra button: no keys, a third key, a stray value.
        assert_eq!(side_key(0), None);
        assert_eq!(side_key(3 << 16), None);
        // The low word carries the buttons held down while the key was pressed — shift, say.
        // It names neither key, and must not be read as one.
        assert_eq!(side_key(0x0001), None);
        assert_eq!(side_key(0x0002), None);
    }

    #[test]
    fn the_message_and_button_numbers_are_the_ones_windows_sends() {
        // Pinned because these numbers are a wire contract: a wrong one changes nothing at compile
        // time and loses the keys at run time. winuser.h of the Windows SDK (10.0.26100.0).
        assert_eq!((WM_XBUTTONDOWN, WM_XBUTTONUP, WM_XBUTTONDBLCLK), (0x020B, 0x020C, 0x020D));
        assert_eq!((XBUTTON1, XBUTTON2), (1, 2));
        // Measured the hard way once: 0x00AB/0x00AC/0x00AD a few lines above in that header are the
        // non-client twins — what the same key over a titlebar would produce. Posting those to a
        // window's client area answers "this message is for synchronous use only" or nothing at all.
        assert_ne!(WM_XBUTTONDOWN, 0x00AB);
    }

    #[test]
    fn a_key_held_with_the_ordinary_buttons_still_names_its_key() {
        // High word XBUTTON1, low word a left button held: the back key still means back.
        assert_eq!(side_key((XBUTTON1 << 16) | 0x0001), Some(SideKey::Back));
    }
}
