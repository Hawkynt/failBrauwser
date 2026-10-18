//! Phones, tablets and cameras on USB, which GVfs reaches over MTP (Android), PTP (cameras)
//! or AFC (iPhone, iPad). They are not block devices: UDisks2 does not know them, GVfs's
//! volume monitors do. This holds what can be said about them without a display: what kind
//! of device an address is, a readable name for one, and what to tell the user when it does
//! not open.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// MTP: Android phones and tablets, some music players.
    Phone,
    /// PTP (gphoto2): cameras, and phones set to send photos only.
    Camera,
    /// AFC: iPhone, iPad.
    Apple,
}

impl DeviceKind {
    /// The kind of device behind a GVfs address (`mtp://…`), if it is one of these.
    pub fn of_uri(uri: &str) -> Option<DeviceKind> {
        let scheme = uri.split_once(':')?.0.to_ascii_lowercase();
        match scheme.as_str() {
            "mtp" => Some(DeviceKind::Phone),
            "gphoto2" => Some(DeviceKind::Camera),
            "afc" => Some(DeviceKind::Apple),
            _ => None,
        }
    }

    /// The kind of device whose files gvfsd-fuse shows in this folder (`mtp:host=…`).
    pub fn of_fuse_dir(name: &str) -> Option<DeviceKind> {
        Self::of_uri(name)
    }

    /// In words, for the drives page and properties.
    pub fn describe(self) -> &'static str {
        match self {
            DeviceKind::Phone => "Phone or tablet (MTP)",
            DeviceKind::Camera => "Camera (PTP)",
            DeviceKind::Apple => "iPhone or iPad",
        }
    }

    /// The protocol, as the drive properties call the connection.
    pub fn protocol(self) -> &'static str {
        match self {
            DeviceKind::Phone => "mtp",
            DeviceKind::Camera => "gphoto2",
            DeviceKind::Apple => "afc",
        }
    }

    pub fn from_protocol(p: &str) -> Option<DeviceKind> {
        Self::of_uri(&format!("{p}:"))
    }

    pub fn icon_names(self) -> Vec<&'static str> {
        match self {
            DeviceKind::Phone | DeviceKind::Apple => vec!["phone", "multimedia-player", "drive-removable-media"],
            DeviceKind::Camera => vec!["camera-photo", "media-flash", "drive-removable-media"],
        }
    }

    pub fn symbolic_icon(self) -> &'static str {
        match self {
            DeviceKind::Phone | DeviceKind::Apple => "phone-symbolic",
            DeviceKind::Camera => "camera-photo-symbolic",
        }
    }
}

/// A readable name from the host part GVfs builds out of the USB vendor, model and serial
/// number: `mtp:host=Google_Pixel_7_28041FDH2000ZA` or `mtp://SAMSUNG_Android_R58M12ABC/`
/// give "Google Pixel 7" and "SAMSUNG Android". Used when GVfs's own name is not at hand.
pub fn name_from_host(text: &str) -> Option<String> {
    DeviceKind::of_uri(text)?;
    let rest = text.split_once(':')?.1;
    let host = rest.strip_prefix("host=").or_else(|| rest.strip_prefix("//")).unwrap_or(rest);
    let host = host.split(['/', ',']).next().unwrap_or_default();
    let host = percent_decode(host);
    // Old GVfs: "[usb:001,005]", nothing to read a name from.
    if host.is_empty() || host.starts_with('[') {
        return None;
    }
    let mut words: Vec<&str> = host.split('_').filter(|w| !w.is_empty()).collect();
    // The serial number at the end: long, with digits, no lower-case letters.
    if words.len() > 1 {
        let last = words[words.len() - 1];
        if last.len() >= 6 && last.chars().any(|c| c.is_ascii_digit()) && !last.chars().any(|c| c.is_ascii_lowercase()) {
            words.pop();
        }
    }
    if words.is_empty() { None } else { Some(words.join(" ")) }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What the user can do about a device that did not open (or shows nothing): the message
/// GVfs gave, then advice for the usual causes.
pub fn explain_error(kind: DeviceKind, msg: &str) -> String {
    let lower = msg.to_lowercase();
    let advice = match kind {
        DeviceKind::Apple => "Unlock the iPhone or iPad and tap “Trust” when it asks whether to trust this computer.",
        DeviceKind::Camera => "Switch the camera on and set it to PC or PTP mode. On a phone, choose “PTP” or “Transfer photos” in its USB notification.",
        DeviceKind::Phone if lower.contains("busy") || lower.contains("unable to open") || lower.contains("claim") => {
            "Another program is probably using the phone (another file manager, a photo importer, KDE Connect…). Close it, unplug the phone and plug it in again."
        }
        DeviceKind::Phone => "Unlock the phone, then pull down its notifications, tap “USB” and choose “File transfer”.",
    };
    if msg.trim().is_empty() { advice.to_string() } else { format!("{}\n\n{advice}", msg.trim()) }
}

/// The line shown for a device that is open but shows no storage: a locked phone, or one
/// that has not allowed access yet.
pub fn no_storage_hint(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Apple => "Nothing to show — unlock it and tap “Trust”",
        DeviceKind::Camera => "Nothing to show — is the camera switched on?",
        DeviceKind::Phone => "Nothing to show — unlock the phone and choose “File transfer”",
    }
}

/// Folders that stand for a device: the root of its files (gvfsd-fuse's folder for a phone),
/// with the name to show for it. Kept up to date by the UI from GVfs's volume monitor, read
/// by tab titles and the path bar.
static NAMED_ROOTS: Mutex<Option<HashMap<PathBuf, (String, DeviceKind)>>> = Mutex::new(None);

/// Replaces the known device folders.
pub fn set_named_roots(roots: Vec<(PathBuf, String, DeviceKind)>) {
    let map = roots.into_iter().map(|(p, n, k)| (p, (n, k))).collect();
    *NAMED_ROOTS.lock().unwrap() = Some(map);
}

/// The device folder `p` lies in, with the device's name and kind. A folder of gvfsd-fuse
/// that GVfs did not name (yet) is named from its host part.
pub fn named_root(p: &Path) -> Option<(PathBuf, String, DeviceKind)> {
    {
        let roots = NAMED_ROOTS.lock().unwrap();
        if let Some(map) = roots.as_ref() {
            let best = map.iter().filter(|(root, _)| p.starts_with(root)).max_by_key(|(root, _)| root.as_os_str().len());
            if let Some((root, (name, kind))) = best {
                return Some((root.clone(), name.clone(), *kind));
            }
        }
    }
    let root = crate::ops::gvfs::share_root(p)?;
    let dir = root.file_name()?.to_string_lossy().into_owned();
    let kind = DeviceKind::of_fuse_dir(&dir)?;
    Some((root, name_from_host(&dir)?, kind))
}

