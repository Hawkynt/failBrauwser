// The `\\?\` extended-path discipline: only ever applied inside this module and
// the osprim/win opens, so every path the UI, jobs and .trashinfo files carry
// stays a plain readable path.

use std::path::{Component, Path, PathBuf};

/// Absolute, backslash-normalized, `\\?\`-prefixed UTF-16 (NUL-terminated) form
/// of `path`: no 260-char limit, no dot-segment magic, no quoting surprises.
pub fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    extended(path).as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// Plain absolute UTF-16 (NUL-terminated) form — the few APIs that parse the drive
/// themselves, like `GetVolumeInformationW`, answer `ERROR_INVALID_NAME` for the
/// `\\?\` form and would silently turn their results into "unknown".
pub fn plain_wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let absolute = if path.is_absolute() { path.to_path_buf() } else { relativize(path) };
    absolute.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

pub fn extended(path: &Path) -> std::ffi::OsString {
    let absolute = if path.is_absolute() { path.to_path_buf() } else { relativize(path) };
    let text = absolute.to_string_lossy().replace('/', "\\");
    if text.starts_with("\\\\?\\") {
        return std::ffi::OsString::from(text);
    }
    std::ffi::OsString::from(if text.starts_with("\\\\") {
        format!("\\\\?\\UNC\\{}", &text[2..])
    } else {
        format!("\\\\?\\{text}")
    })
}

/// `std` only joins the current directory for relative paths; keep it explicit.
fn relativize(path: &Path) -> PathBuf {
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut out = base;
    for c in path.components() {
        match c {
            Component::Normal(part) => out.push(part),
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            _ => {}
        }
    }
    out
}
