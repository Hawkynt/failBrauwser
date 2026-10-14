//! Thumbnails as the freedesktop.org Thumbnail Managing Standard defines them, so they are
//! shared with Thunar, Nautilus and every other program that follows it:
//! `$XDG_CACHE_HOME/thumbnails/{normal,large}/<md5 of the file URI>.png`, with the file's URI
//! and modification time stored in the PNG (`Thumb::URI`, `Thumb::MTime`). A thumbnail whose
//! `MTime` differs from the file's is stale.
//!
//! Most thumbnails are made by the desktop's thumbnailer service (Tumbler, the one Thunar
//! uses); this module reads the cache and makes image thumbnails itself when no service is
//! there.

use gtk::gdk_pixbuf::Pixbuf;
use gtk::glib;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flavor {
    /// 128 × 128
    Normal,
    /// 256 × 256
    Large,
}

impl Flavor {
    pub fn size(self) -> i32 {
        match self {
            Flavor::Normal => 128,
            Flavor::Large => 256,
        }
    }
    pub fn dir(self) -> &'static str {
        match self {
            Flavor::Normal => "normal",
            Flavor::Large => "large",
        }
    }
    pub fn name(self) -> &'static str {
        self.dir()
    }
    /// The smallest flavor that looks sharp at `px` pixels.
    pub fn for_size(px: i32) -> Flavor {
        if px > 128 { Flavor::Large } else { Flavor::Normal }
    }
}

/// `$XDG_CACHE_HOME/thumbnails`
pub fn cache_root() -> PathBuf {
    glib::user_cache_dir().join("thumbnails")
}

pub fn file_uri(path: &Path) -> Option<String> {
    glib::filename_to_uri(path, None).ok().map(|u| u.to_string())
}

fn md5(uri: &str) -> String {
    glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri).map(|s| s.to_string()).unwrap_or_default()
}

pub fn thumb_path(root: &Path, uri: &str, flavor: Flavor) -> PathBuf {
    root.join(flavor.dir()).join(format!("{}.png", md5(uri)))
}

/// Where we note files we could not thumbnail, so they are not tried again and again.
pub fn fail_path(root: &Path, uri: &str) -> PathBuf {
    root.join("fail/failbrauwser").join(format!("{}.png", md5(uri)))
}

/// A thumbnail file belongs to this version of the file.
fn is_current(thumb: &Pixbuf, uri: &str, mtime: i64) -> bool {
    let uri_ok = thumb.option("tEXt::Thumb::URI").is_none_or(|u| u == uri);
    // Whole seconds per the spec; newer Tumbler versions add fractions ("1790670134.968127").
    let mtime_ok = thumb.option("tEXt::Thumb::MTime").and_then(|m| m.parse::<f64>().ok()).map(|m| m.floor() as i64) == Some(mtime);
    uri_ok && mtime_ok
}

/// A current thumbnail from the cache: the wanted flavor, else the other one. Also says
/// which flavor it is (a smaller one than wanted looks blurry or lost in its space).
pub fn load(root: &Path, path: &Path, mtime: i64, flavor: Flavor) -> Option<(Pixbuf, Flavor)> {
    let uri = file_uri(path)?;
    let order = if flavor == Flavor::Large { [Flavor::Large, Flavor::Normal] } else { [Flavor::Normal, Flavor::Large] };
    for f in order {
        let p = thumb_path(root, &uri, f);
        if let Ok(pb) = Pixbuf::from_file(&p) {
            if is_current(&pb, &uri, mtime) {
                return Some((pb, f));
            }
        }
    }
    None
}

/// We already failed on this version of the file.
pub fn failed_before(root: &Path, path: &Path, mtime: i64) -> bool {
    let Some(uri) = file_uri(path) else { return false };
    Pixbuf::from_file(fail_path(root, &uri)).is_ok_and(|pb| is_current(&pb, &uri, mtime))
}

fn save(pb: &Pixbuf, dest: &Path, uri: &str, mtime: i64) -> bool {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let Some(dir) = dest.parent() else { return false };
    if std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).is_err() {
        return false;
    }
    // Written under a temporary name and renamed: readers never see half a file.
    // Unique per call: several workers save at once.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(".{}-{n}.fb-tmp", std::process::id()));
    let mtime_s = mtime.to_string();
    let ok = pb
        .savev(&tmp, "png", &[("tEXt::Thumb::URI", uri), ("tEXt::Thumb::MTime", &mtime_s), ("tEXt::Software", "failBrauwser")])
        .is_ok();
    if ok {
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        return std::fs::rename(&tmp, dest).is_ok();
    }
    let _ = std::fs::remove_file(&tmp);
    false
}

/// Makes an image thumbnail with GdkPixbuf and stores it in the cache. On failure a fail
/// marker is written. Only for images; other types need a thumbnailer service.
pub fn generate(root: &Path, path: &Path, mtime: i64, flavor: Flavor) -> Option<Pixbuf> {
    let uri = file_uri(path)?;
    let size = flavor.size();
    let result = Pixbuf::from_file_at_scale(path, size, size, true).ok().map(|pb| {
        // Small images are not enlarged; EXIF rotation is applied.
        pb.apply_embedded_orientation().unwrap_or(pb)
    });
    match result {
        Some(pb) => {
            save(&pb, &thumb_path(root, &uri, flavor), &uri, mtime);
            Some(pb)
        }
        None => {
            if let Some(marker) = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, 1, 1) {
                save(&marker, &fail_path(root, &uri), &uri, mtime);
            }
            None
        }
    }
}

/// Worth asking for a thumbnail at all (by MIME type).
pub fn is_candidate(content_type: &str) -> bool {
    content_type.starts_with("image/")
        || content_type.starts_with("video/")
        || content_type.starts_with("font/")
        || matches!(
            content_type,
            "application/pdf"
                | "application/epub+zip"
                | "application/x-cbz"
                | "application/x-cbr"
                | "application/vnd.oasis.opendocument.text"
                | "application/vnd.oasis.opendocument.spreadsheet"
                | "application/vnd.oasis.opendocument.presentation"
                | "application/vnd.oasis.opendocument.graphics"
                | "application/x-font-ttf"
                | "application/x-font-otf"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn png(path: &Path, w: i32, h: i32) {
        let pb = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, w, h).unwrap();
        pb.fill(0x3366ccff);
        pb.savev(path, "png", &[]).unwrap();
    }

    fn mtime(p: &Path) -> i64 {
        std::fs::metadata(p).unwrap().mtime()
    }

    #[test]
    fn paths_follow_the_spec() {
        // md5("file:///home/jens/photos/me.png") from the specification's example.
        let p = thumb_path(Path::new("/c"), "file:///home/jens/photos/me.png", Flavor::Normal);
        assert_eq!(p, PathBuf::from("/c/normal/c6ee772d9e49320e97ec29a7eb5b1697.png"));
        assert_eq!(Flavor::for_size(256), Flavor::Large);
        assert_eq!(Flavor::for_size(96), Flavor::Normal);
    }

    #[test]
    fn generates_stores_and_reloads() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("thumbs");
        let img = d.path().join("photo.png");
        png(&img, 800, 400);
        let m = mtime(&img);
        assert!(load(&root, &img, m, Flavor::Normal).is_none());
        let t = generate(&root, &img, m, Flavor::Normal).unwrap();
        assert_eq!((t.width(), t.height()), (128, 64));
        let uri = file_uri(&img).unwrap();
        let stored = thumb_path(&root, &uri, Flavor::Normal);
        assert!(stored.is_file());
        assert_eq!(std::fs::metadata(&stored).unwrap().mode() & 0o777, 0o600);
        let (back, f) = load(&root, &img, m, Flavor::Normal).unwrap();
        assert_eq!(back.option("tEXt::Thumb::URI").as_deref(), Some(uri.as_str()));
        assert_eq!(f, Flavor::Normal);
        // A large request can make do with the normal one until a large exists, and says so.
        assert_eq!(load(&root, &img, m, Flavor::Large).map(|(_, f)| f), Some(Flavor::Normal));
        let big = generate(&root, &img, m, Flavor::Large).unwrap();
        assert_eq!((big.width(), big.height()), (256, 128));
        assert_eq!(load(&root, &img, m, Flavor::Large).map(|(_, f)| f), Some(Flavor::Large));
        // A changed file makes the thumbnail stale.
        assert!(load(&root, &img, m + 1, Flavor::Normal).is_none());
    }

    #[test]
    fn fractional_mtimes_from_other_thumbnailers_are_accepted() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("thumbs");
        let img = d.path().join("x.png");
        png(&img, 64, 64);
        let m = mtime(&img);
        let uri = file_uri(&img).unwrap();
        let pb = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, 64, 64).unwrap();
        std::fs::create_dir_all(root.join("normal")).unwrap();
        let frac = format!("{m}.968127");
        pb.savev(thumb_path(&root, &uri, Flavor::Normal), "png", &[("tEXt::Thumb::URI", uri.as_str()), ("tEXt::Thumb::MTime", frac.as_str())]).unwrap();
        assert!(load(&root, &img, m, Flavor::Normal).is_some());
        assert!(load(&root, &img, m - 1, Flavor::Normal).is_none());
    }

    #[test]
    fn broken_images_are_remembered() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("thumbs");
        let bad = d.path().join("bad.png");
        std::fs::write(&bad, b"not a png").unwrap();
        let m = mtime(&bad);
        assert!(generate(&root, &bad, m, Flavor::Normal).is_none());
        assert!(failed_before(&root, &bad, m));
        assert!(!failed_before(&root, &bad, m + 5));
    }

    #[test]
    fn candidates() {
        assert!(is_candidate("image/jpeg"));
        assert!(is_candidate("video/mp4"));
        assert!(is_candidate("application/pdf"));
        assert!(!is_candidate("text/plain"));
    }
}
