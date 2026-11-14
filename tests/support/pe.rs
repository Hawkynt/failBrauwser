//! The part of a Windows program file the shell reads: the subsystem it is started under and
//! the resource types it carries. Shared by the scenarios that pin the icon and those that pin
//! how the program starts; `tests/*.rs` compile separately, so each includes this file.

#![allow(dead_code)] // each target uses only part of what the other scenarios need.

use std::path::Path;

pub const SUBSYSTEM_GUI: u16 = 2;
pub const SUBSYSTEM_CONSOLE: u16 = 3;

/// The subsystem a program file asks to start under: 2 is a windowed program (Windows
/// allocates it no console), 3 is a console program.
pub fn subsystem(path: &Path) -> Result<u16, String> {
    let pe = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let h = headers(&pe)?;
    Ok(read_u16(&pe, h.opt + 68))
}

/// The resource types a program file carries, by their numeric kind (RT_ICON is 3,
/// RT_GROUP_ICON 14, VERSION 16, MANIFEST 24).
pub fn resource_kinds(pe: &[u8]) -> Result<Vec<u16>, String> {
    let h = headers(pe)?;
    let dirs = match read_u16(pe, h.opt) {
        0x10b => h.opt + 96,
        0x20b => h.opt + 112,
        m => return Err(format!("unexpected optional header magic {m:#x}")),
    };
    let root = h.file_offset(pe, read_u32(pe, dirs + 2 * 8) as usize)?;
    let (named, ids) = (read_u16(pe, root + 12) as usize, read_u16(pe, root + 14) as usize);
    let mut kinds = Vec::with_capacity(ids);
    for i in named..named + ids {
        kinds.push(read_u16(pe, root + 16 + i * 8) & 0x7fff);
    }
    Ok(kinds)
}

struct Headers {
    opt: usize,
    secs: usize,
    count: usize,
}

impl Headers {
    /// Where an RVA lies in the file, found through the section table.
    fn file_offset(&self, pe: &[u8], rva: usize) -> Result<usize, String> {
        for i in 0..self.count {
            let s = self.secs + i * 40;
            let (va, len, raw) = (read_u32(pe, s + 12) as usize, read_u32(pe, s + 16) as usize, read_u32(pe, s + 20) as usize);
            if rva >= va && rva < va + len {
                return Ok(raw + (rva - va));
            }
        }
        Err(format!("no section covers rva {rva:x}"))
    }
}

fn headers(pe: &[u8]) -> Result<Headers, String> {
    // 0x3c holds a pointer to the COFF header, which starts with the "PE\0\0" signature.
    let lfanew = read_u32(pe, 0x3c) as usize;
    if slice(pe, lfanew, 4)? != b"PE\0\0" {
        return Err("no PE header".into());
    }
    let (opt, secs) = (lfanew + 24, lfanew + 24 + read_u16(pe, lfanew + 20) as usize);
    Ok(Headers { opt, secs, count: read_u16(pe, lfanew + 6) as usize })
}

fn read_u16(b: &[u8], at: usize) -> u16 { u16::from_le_bytes(slice(b, at, 2).unwrap().try_into().unwrap()) }

fn read_u32(b: &[u8], at: usize) -> u32 { u32::from_le_bytes(slice(b, at, 4).unwrap().try_into().unwrap()) }

fn slice(b: &[u8], at: usize, len: usize) -> Result<&[u8], String> {
    b.get(at..at + len).ok_or_else(|| format!("the file ends at byte {at}"))
}
