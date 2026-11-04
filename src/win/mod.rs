// Windows internals. Everything Win32/COM-specific lives here or in the
// `.windows.rs` module arms; shared code reaches it through `osprim` or a
// `#[cfg(windows)]` call — never through ad-hoc re-derivations.
pub mod err;
pub mod longpath;
pub mod netinfo;
pub mod recyclebin;
pub mod storage;
pub mod volume;
