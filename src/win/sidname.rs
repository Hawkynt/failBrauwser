//! Owner and group names of a file: the security descriptor carries two SIDs, and this
//! turns them into the names the user knows — the same two the Security tab shows.
//! Resolved names are cached per SID string, because one folder is almost always owned
//! by one or two accounts over and over.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use windows::core::{BOOL, PWSTR, PCWSTR};
use windows::Win32::Foundation::{ERROR_SUCCESS, HLOCAL, LocalFree};
use windows::Win32::Security::{GetSecurityDescriptorGroup, GetSecurityDescriptorOwner, PSECURITY_DESCRIPTOR, PSID, SID_NAME_USE, GROUP_SECURITY_INFORMATION, LookupAccountSidW, OWNER_SECURITY_INFORMATION};
use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows::Win32::System::WindowsProgramming::GetUserNameW;

/// The signed-in account's name (an empty string if the API refuses).
pub fn current_user() -> String {
    unsafe {
        let mut cch = 256u32;
        let mut buf = vec![0u16; cch as usize];
        match GetUserNameW(Some(PWSTR(buf.as_mut_ptr())), &raw mut cch) {
            Ok(()) => decode(&buf),
            Err(_) => String::new(),
        }
    }
}

/// The owner and group names of `path`. An account the directory no longer knows stays
/// its SID string, which is what Explorer shows in the same case.
pub fn owner_group(path: &Path) -> io::Result<(String, String)> {
    unsafe {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        // Ask for the whole descriptor, not the loose SIDs: the two SIDs live inside the
        // one block it allocates, and freeing a SID on its own corrupts the heap
        // (probe-verified here: the process died with STATUS_HEAP_CORRUPTION that way).
        let mut sd = PSECURITY_DESCRIPTOR::default();
        let out = GetNamedSecurityInfoW(PCWSTR(wide.as_ptr()), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION, None, None, None, None, &raw mut sd);
        if out != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(out.0 as i32));
        }
        let (mut owner, mut group) = (PSID::default(), PSID::default());
        let (mut odefaulted, mut gdefaulted) = (BOOL::default(), BOOL::default());
        let read = GetSecurityDescriptorOwner(sd, &raw mut owner, &raw mut odefaulted).and(GetSecurityDescriptorGroup(sd, &raw mut group, &raw mut gdefaulted));
        let (o, g) = (name_of(owner), name_of(group));
        // The one block the call allocated, freed exactly once; the SIDs were parts of it.
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        read.map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))?;
        Ok((o.ok_or_else(|| io::Error::other("the file carries no owner"))?, g.ok_or_else(|| io::Error::other("the file carries no group"))?))
    }
}

/// One SID's friendly name, cached; its SID string when no account answers (or, as a
/// last resort, when even the SID cannot be printed).
fn name_of(psid: PSID) -> Option<String> {
    if psid.0.is_null() {
        return None;
    }
    unsafe {
        let mut sidp = PWSTR::default();
        if ConvertSidToStringSidW(psid, &raw mut sidp).is_err() {
            return None;
        }
        // A SID string tops out near 68 characters; scan to its NUL rather than trust a bound.
        let mut units = Vec::with_capacity(70);
        for i in 0..128isize {
            let c = *sidp.0.offset(i);
            if c == 0 {
                break;
            }
            units.push(c);
        }
        let key = String::from_utf16_lossy(&units);
        let name = match cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
            Some(name) => name,
            None => {
                let (mut cch, mut dcch) = (256u32, 256u32);
                let mut name = vec![0u16; 256];
                let mut domain = vec![0u16; 256];
                let mut use_ = SID_NAME_USE::default();
                match LookupAccountSidW(PCWSTR::null(), psid, Some(PWSTR(name.as_mut_ptr())), &raw mut cch, Some(PWSTR(domain.as_mut_ptr())), &raw mut dcch, &raw mut use_) {
                    Ok(()) => {
                        let found = decode(&name);
                        if let Ok(mut c) = cache().lock() {
                            c.insert(key.clone(), found.clone());
                        }
                        found
                    }
                    Err(_) => {
                                        key.clone()
                    },
                }
            }
        };
        let _ = LocalFree(Some(HLOCAL(sidp.0.cast())));
        Some(name)
    }
}

/// SID string → resolved name, shared across the loader threads.
static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, String>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn decode(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_i_made_is_owned_by_me_and_named() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("owned-by-me.txt");
        std::fs::write(&file, b"x").unwrap();
        let (owner, group) = owner_group(&file).expect("a fresh file answers its owner and group");
        assert!(!owner.is_empty() && !group.is_empty(), "both names print ({owner} / {group})");
        assert_eq!(owner.to_lowercase(), current_user().to_lowercase(), "I own the file I just made");
        let again = owner_group(&file).unwrap();
        assert_eq!(again.0, owner, "the cache answers the same name");
    }

    #[test]
    fn a_gone_file_names_its_error_not_a_panic() {
        let e = owner_group(std::path::Path::new("A:\\no-such-folder-here\\gone.txt")).unwrap_err();
        assert!(e.raw_os_error().is_some(), "the missing file reports its Win32 code");
    }
}
