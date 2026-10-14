//! Temporary folders of jobs (archive contents on their way to a destination): each one is
//! recorded with the process that made it, so what a crashed or killed run left behind is
//! removed at the next start. The record is `$XDG_CACHE_HOME/failbrauwser/temp-folders`, one
//! `pid<TAB>path` per line.
//!
//! Only folders whose contents exist elsewhere too are recorded: never the copies of files
//! opened from archives, which may hold edits not saved back yet.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

static LOCK: Mutex<()> = Mutex::new(());

pub fn registry_path() -> PathBuf {
    gtk::glib::user_cache_dir().join("failbrauwser").join("temp-folders")
}

/// Records a temporary folder of this process.
pub fn created(dir: &Path) {
    let _ = track_in(&registry_path(), dir, std::process::id());
}

/// Removes a temporary folder and its record.
pub fn remove(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
    let _ = untrack_in(&registry_path(), dir);
}

/// Removes the folders that processes no longer running left behind. Returns them.
pub fn clean_stale() -> Vec<PathBuf> {
    let own = comm(std::process::id());
    clean_stale_in(&registry_path(), |pid| own.is_some() && comm(pid) == own)
}

/// The command name of a process, if it runs.
fn comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim().to_string())
}

fn read(reg: &Path) -> Vec<(u32, PathBuf)> {
    std::fs::read_to_string(reg)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (pid, path) = l.split_once('\t')?;
            Some((pid.parse().ok()?, PathBuf::from(path)))
        })
        .collect()
}

fn write(reg: &Path, entries: &[(u32, PathBuf)]) -> io::Result<()> {
    if let Some(dir) = reg.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text: String = entries.iter().map(|(pid, p)| format!("{pid}\t{}\n", p.display())).collect();
    std::fs::write(reg, text)
}

fn track_in(reg: &Path, dir: &Path, pid: u32) -> io::Result<()> {
    let _g = LOCK.lock().unwrap();
    let mut e = read(reg);
    e.retain(|(_, p)| p != dir);
    e.push((pid, dir.to_path_buf()));
    write(reg, &e)
}

fn untrack_in(reg: &Path, dir: &Path) -> io::Result<()> {
    let _g = LOCK.lock().unwrap();
    let mut e = read(reg);
    let before = e.len();
    e.retain(|(_, p)| p != dir);
    if e.len() != before { write(reg, &e) } else { Ok(()) }
}

/// Only folders that look like ours are ever removed, whatever the record says.
fn ours(p: &Path) -> bool {
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    name.starts_with(".fb-extract-") || (name.starts_with("fb-") && p.ancestors().any(|a| a.file_name().is_some_and(|n| n == "failbrauwser")))
}

fn clean_stale_in(reg: &Path, alive: impl Fn(u32) -> bool) -> Vec<PathBuf> {
    let _g = LOCK.lock().unwrap();
    let mut keep = Vec::new();
    let mut removed = Vec::new();
    for (pid, p) in read(reg) {
        if alive(pid) {
            keep.push((pid, p));
            continue;
        }
        // A share that does not answer would hang the start; try again next time.
        if super::gvfs::check(&p, Duration::from_secs(2)).is_err_and(|e| super::gvfs::not_responding_share(&e).is_some()) {
            keep.push((pid, p));
            continue;
        }
        if !ours(&p) || std::fs::symlink_metadata(&p).is_err() {
            continue;
        }
        match std::fs::remove_dir_all(&p) {
            Ok(()) => removed.push(p),
            Err(_) => keep.push((pid, p)),
        }
    }
    let _ = write(reg, &keep);
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leftovers_of_dead_processes_go_the_rest_stays() {
        let dir = tempfile::tempdir().unwrap();
        let reg = dir.path().join("temp-folders");
        let mk = |rel: &str| {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.join("inner")).unwrap();
            std::fs::write(p.join("inner/f"), "x").unwrap();
            p
        };
        let dead_extract = mk("dest/.fb-extract-abc");
        let dead_stage = mk("cache/failbrauwser/stage/fb-123");
        let live = mk("dest/.fb-extract-live");
        let not_ours = mk("dest/photos");
        track_in(&reg, &dead_extract, 111).unwrap();
        track_in(&reg, &dead_stage, 111).unwrap();
        track_in(&reg, &live, 222).unwrap();
        track_in(&reg, &not_ours, 111).unwrap();
        track_in(&reg, &dir.path().join("dest/.fb-extract-gone"), 111).unwrap();

        let removed = clean_stale_in(&reg, |pid| pid == 222);
        assert_eq!(removed, vec![dead_extract.clone(), dead_stage.clone()]);
        assert!(!dead_extract.exists() && !dead_stage.exists());
        assert!(live.exists());
        assert!(not_ours.join("inner/f").exists(), "only our own temp folders are removed");
        assert_eq!(read(&reg), vec![(222, live.clone())]);

        // Finished jobs take their record away.
        untrack_in(&reg, &live).unwrap();
        assert!(read(&reg).is_empty());
    }
}
