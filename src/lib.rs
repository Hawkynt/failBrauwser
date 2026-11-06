//! failBrauwser core library: everything that does not need a display lives here
//! so it can be unit tested headless.

pub mod actions;
pub mod appearance;
pub mod archive;
pub mod chooser;
pub mod clipformat;
pub mod fs;
pub mod config;
pub mod desktoplink;
pub mod devices;
pub mod drives;
pub mod location;
#[cfg(unix)]
pub mod lvm;
/// LVM inside encrypted drives is a Linux matter (device-mapper, UDisks2, pkexec); the
/// shared UDisks2 reading still names the type, and finds no LVM volumes on Windows.
#[cfg(not(unix))]
pub mod lvm {
    pub const PV_TYPE: &str = "LVM2_member";
    pub fn lvm_containers(_sys: &std::path::Path, devices: &[String], _containers: &[(String, String)]) -> Vec<Option<String>> {
        vec![None; devices.len()]
    }
}
pub mod network;
pub mod ops;
pub mod osprim;
#[cfg(windows)]
pub mod remote_fs;
pub mod remote;
pub mod search;
pub mod servers;
pub mod shortcuts;
pub mod thumbs;
pub mod trash;
pub mod update;
pub mod wmtheme;
#[cfg(windows)]
pub mod win;
#[cfg(windows)]
pub mod wpd;
