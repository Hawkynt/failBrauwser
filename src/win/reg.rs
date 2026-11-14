//! The handful of registry values the app itself touches: its autostart entry in the
//! per-user Run key, and the desktop's light-or-dark choice (read-only, Windows 10+).
//! Everything stays under HKCU — no elevation, nothing system-wide.

use std::io;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_SZ, REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const AUTOSTART_VALUE: &str = "failBrauwser";

/// Whether Windows wants light app colours (`None`: the desktop spells no preference).
pub fn apps_use_light_theme() -> Option<bool> {
    read_dword(HKEY_CURRENT_USER, PERSONALIZE, "AppsUseLightTheme").map(|v| v != 0)
}

/// The autostart command currently registered, if the app set one.
pub fn autostart_command() -> Option<String> {
    read_string(HKEY_CURRENT_USER, RUN, AUTOSTART_VALUE)
}

/// Registers (or replaces) the autostart entry; `command` is the quoted exe plus `--daemon`.
pub fn set_autostart(command: &str) -> io::Result<()> {
    write_string(HKEY_CURRENT_USER, RUN, AUTOSTART_VALUE, command)
}

/// Removes the autostart entry; absent is success (the button is a toggle, not a probe).
pub fn clear_autostart() -> io::Result<()> {
    unsafe {
        let out = RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN), &HSTRING::from(AUTOSTART_VALUE));
        if out == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        win32_err(out)
    }
}

fn read_dword(root: HKEY, path: &str, value: &str) -> Option<u32> {
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(root, &HSTRING::from(path), None, KEY_READ, &raw mut key).ok().ok()?;
        let mut vtype = REG_VALUE_TYPE::default();
        let mut data = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let out = RegQueryValueExW(key, &HSTRING::from(value), None, Some(&raw mut vtype), Some((&raw mut data) as *mut u8), Some(&raw mut size));
        let _ = RegCloseKey(key);
        (out.is_ok() && vtype == REG_DWORD).then_some(data)
    }
}

fn read_string(root: HKEY, path: &str, value: &str) -> Option<String> {
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(root, &HSTRING::from(path), None, KEY_READ, &raw mut key).ok().ok()?;
        let mut vtype = REG_VALUE_TYPE::default();
        let mut buf = vec![0u16; 2_048];
        let mut size = (buf.len() * 2) as u32;
        let out = RegQueryValueExW(key, &HSTRING::from(value), None, Some(&raw mut vtype), Some(buf.as_mut_ptr() as *mut u8), Some(&raw mut size));
        let _ = RegCloseKey(key);
        if out.is_err() || vtype != REG_SZ {
            return None;
        }
        let wide = &buf[..(size as usize / 2).min(buf.len())];
        String::from_utf16(wide.strip_suffix(&[0u16]).unwrap_or(wide)).ok()
    }
}

fn write_string(root: HKEY, path: &str, value: &str, data: &str) -> io::Result<()> {
    unsafe {
        let mut key = HKEY::default();
        win32_err(RegCreateKeyExW(root, &HSTRING::from(path), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &raw mut key, None))?;
        let wide: Vec<u16> = data.encode_utf16().collect();
        let out = RegSetValueExW(key, &HSTRING::from(value), None, REG_SZ, Some(by_slice(wide.as_slice())));
        let _ = RegCloseKey(key);
        win32_err(out)
    }
}

fn by_slice(b: &[u16]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(b.as_ptr() as *const u8, b.len() * 2) }
}

fn win32_err(e: WIN32_ERROR) -> io::Result<()> {
    if e.is_ok() {
        return Ok(());
    }
    Err(io::Error::from_raw_os_error(e.0 as i32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Registry::RegDeleteTreeW;

    #[test]
    fn values_round_trip_under_the_apps_own_key_and_vanish_after() {
        // A key only this test ever touches, in the user's own hive: hermetic and cleanup-verifiable.
        let path = r"Software\failBrauwser-test-probe";
        write_string(HKEY_CURRENT_USER, path, "probe", "v1").expect("HKCU writes need no elevation");
        assert_eq!(read_string(HKEY_CURRENT_USER, path, "probe").as_deref(), Some("v1"));
        write_string(HKEY_CURRENT_USER, path, "probe", "second").expect("replace");
        assert_eq!(read_string(HKEY_CURRENT_USER, path, "probe").as_deref(), Some("second"), "the value is replaced, not appended");
        assert_eq!(read_dword(HKEY_CURRENT_USER, path, "probe"), None, "a string is not a number");
        assert_eq!(read_string(HKEY_CURRENT_USER, path, "nobody-homes-here"), None, "absent reads report nothing");
        let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(path)) };
        assert_eq!(read_string(HKEY_CURRENT_USER, path, "probe"), None, "the test key is gone, not hidden");
    }

    #[test]
    fn the_desktop_theme_answers_its_own_question_or_none() {
        // Whatever this machine is set to, reading must not fail and must not write.
        let _ = apps_use_light_theme();
        let _ = autostart_command();
    }
}
