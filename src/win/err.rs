use std::io;
use windows::core::PCWSTR;
use windows::Win32::Foundation::GetLastError;

/// The last Win32 error as an io::Error, so the errno-as-control-flow predicates
/// (`osprim::is_*`) work the same way on both platforms.
pub fn last() -> io::Error {
    io::Error::from_raw_os_error(unsafe { GetLastError() }.0 as i32)
}

/// A windows crate error carries the Win32 code in its low 16 bits (HRESULT_FROM_WIN32).
pub fn to_io(e: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error((e.code().0 as u32 & 0xFFFF) as i32)
}

/// UTF-16 with a trailing NUL, ready for a PCWSTR argument.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn pcw(s: &[u16]) -> PCWSTR {
    PCWSTR(s.as_ptr())
}
