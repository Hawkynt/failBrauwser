//! Finding duplicates: files with the same bytes, and pictures that show the same thing even
//! at another resolution or saved again (compared by what they show, not by their metadata).

use crate::osprim;
use gtk::gdk_pixbuf::{InterpType, Pixbuf};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A file found while walking a folder.
#[derive(Debug, Clone)]
pub struct Found {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: i64,
}

/// Every regular file below `root` (links are not followed).
pub fn walk(root: &Path, cancel: &AtomicBool) -> Vec<Found> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else if m.is_file() {
                out.push(Found { path: e.path(), size: m.len(), mtime: osprim::times(&m).1.0 });
            }
        }
    }
    out
}

fn hash_file(p: &Path, limit: Option<u64>) -> io::Result<[u8; 32]> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut left = limit.unwrap_or(u64::MAX);
    while left > 0 {
        let want = buf.len().min(left as usize);
        let n = f.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        left -= n as u64;
    }
    Ok(h.finalize().into())
}

/// Groups of files with the same bytes (empty files left out): by size, then by the first
/// 64 KiB, then by everything. `progress` gets how many files were read.
pub fn exact_groups(files: &[Found], cancel: &AtomicBool, progress: &AtomicUsize) -> Vec<Vec<Found>> {
    let mut by_size: HashMap<u64, Vec<&Found>> = HashMap::new();
    for f in files.iter().filter(|f| f.size > 0) {
        by_size.entry(f.size).or_default().push(f);
    }
    let mut groups = Vec::new();
    for same_size in by_size.into_values().filter(|v| v.len() > 1) {
        let mut by_head: HashMap<[u8; 32], Vec<&Found>> = HashMap::new();
        for f in same_size {
            if cancel.load(Ordering::Relaxed) {
                return Vec::new();
            }
            if let Ok(h) = hash_file(&f.path, Some(64 * 1024)) {
                by_head.entry(h).or_default().push(f);
            }
            progress.fetch_add(1, Ordering::Relaxed);
        }
        for same_head in by_head.into_values().filter(|v| v.len() > 1) {
            let mut by_all: HashMap<[u8; 32], Vec<Found>> = HashMap::new();
            for f in same_head {
                let h = if f.size <= 64 * 1024 { hash_file(&f.path, Some(64 * 1024)) } else { hash_file(&f.path, None) };
                if let Ok(h) = h {
                    by_all.entry(h).or_default().push(f.clone());
                }
            }
            groups.extend(by_all.into_values().filter(|v| v.len() > 1));
        }
    }
    for g in &mut groups {
        g.sort_by(|a, b| a.path.cmp(&b.path));
    }
    groups.sort_by(|a, b| a[0].path.cmp(&b[0].path));
    groups
}

/// What a picture shows, small enough to compare thousands: the brightness gradient of a
/// 9×8 grey version (64 bits), an 8×8 colour version, and the size of the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub dhash: u64,
    pub thumb: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Picture suffixes worth decoding.
pub fn is_picture_name(name: &str) -> bool {
    const EXTS: &[&str] = &[".jpg", ".jpeg", ".png", ".webp", ".gif", ".bmp", ".tif", ".tiff", ".heic", ".heif", ".avif", ".jxl"];
    let lower = name.to_lowercase();
    EXTS.iter().any(|e| lower.ends_with(e))
}

/// The pixels of `pb` scaled to `w`×`h`, as RGB (alpha laid on white).
fn rgb(pb: &Pixbuf, w: i32, h: i32) -> Option<Vec<u8>> {
    let small = pb.scale_simple(w, h, InterpType::Bilinear)?;
    let (n, stride) = (small.n_channels() as usize, small.rowstride() as usize);
    let bytes = small.read_pixel_bytes();
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let p = &bytes[y * stride + x * n..];
            let a = if n == 4 { p[3] as u32 } else { 255 };
            for c in 0..3 {
                out.push(((p[c] as u32 * a + 255 * (255 - a)) / 255) as u8);
            }
        }
    }
    Some(out)
}

/// Fingerprints of a picture already decoded.
pub fn fingerprint_of(pb: &Pixbuf, width: u32, height: u32) -> Option<Fingerprint> {
    let grey9: Vec<u32> = rgb(pb, 9, 8)?.chunks(3).map(|p| (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000).collect();
    let mut dhash = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            dhash = dhash << 1 | (grey9[y * 9 + x] > grey9[y * 9 + x + 1]) as u64;
        }
    }
    Some(Fingerprint { dhash, thumb: rgb(pb, 8, 8)?, width, height })
}

/// Decodes a picture (small: the decoder scales while reading) and takes its fingerprints.
pub fn fingerprint(path: &Path) -> Option<Fingerprint> {
    let (_, w, h) = Pixbuf::file_info(path)?;
    let pb = Pixbuf::from_file_at_scale(path, 128, 128, true).ok()?;
    let pb = pb.apply_embedded_orientation().unwrap_or(pb);
    // Turned by its orientation tag: the size as shown.
    let (w, h) = if pb.width() > pb.height() && w < h || pb.width() < pb.height() && w > h { (h, w) } else { (w, h) };
    fingerprint_of(&pb, w as u32, h as u32)
}

/// Whether two pictures show the same: nearly the same brightness pattern and colours, and
/// the same shape (a crop or another picture of the same scene is not the same).
pub fn similar(a: &Fingerprint, b: &Fingerprint) -> bool {
    let ar = |f: &Fingerprint| f.width as f64 / f.height.max(1) as f64;
    if (ar(a) / ar(b) - 1.0).abs() > 0.04 {
        return false;
    }
    let bits = (a.dhash ^ b.dhash).count_ones();
    let colour = a.thumb.iter().zip(&b.thumb).map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs()).sum::<u32>() / a.thumb.len().max(1) as u32;
    bits <= 10 && colour <= 14
}

/// Groups of pictures that show the same, as indexes into `prints` (each group two or more).
pub fn similar_groups(prints: &[Fingerprint]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..prints.len()).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for i in 0..prints.len() {
        for j in i + 1..prints.len() {
            if similar(&prints[i], &prints[j]) {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..prints.len() {
        let r = root(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut out: Vec<Vec<usize>> = groups.into_values().filter(|g| g.len() > 1).collect();
    out.sort();
    out
}

/// Fingerprints of many pictures on all processors; `progress` counts them.
pub fn fingerprints(paths: &[PathBuf], cancel: &AtomicBool, progress: &AtomicUsize) -> Vec<Option<Fingerprint>> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    let next = AtomicUsize::new(0);
    let mut out: Vec<Option<Fingerprint>> = vec![None; paths.len()];
    let results = std::sync::Mutex::new(&mut out);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= paths.len() || cancel.load(Ordering::Relaxed) {
                    break;
                }
                let f = fingerprint(&paths[i]);
                results.lock().unwrap()[i] = f;
                progress.fetch_add(1, Ordering::Relaxed);
            });
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::gdk_pixbuf::Colorspace;

    /// A picture: soft shapes in colour, `variant` moves them.
    fn picture(w: i32, h: i32, variant: i32) -> Pixbuf {
        let pb = Pixbuf::new(Colorspace::Rgb, false, 8, w, h).unwrap();
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f64 / w as f64, y as f64 / h as f64);
                let r = ((fx * 6.0 + variant as f64).sin() * 0.5 + 0.5) * 255.0;
                let g = ((fy * 5.0 - variant as f64 * 1.7).cos() * 0.5 + 0.5) * 255.0;
                let b = (((fx + fy) * 4.0 + variant as f64 * 2.3).sin() * 0.5 + 0.5) * 255.0;
                pb.put_pixel(x as u32, y as u32, r as u8, g as u8, b as u8, 255);
            }
        }
        pb
    }

    #[test]
    fn same_picture_at_other_sizes_and_qualities() {
        let dir = tempfile::tempdir().unwrap();
        let save = |pb: &Pixbuf, name: &str, kind: &str, opts: &[(&str, &str)]| {
            let p = dir.path().join(name);
            pb.savev(&p, kind, opts).unwrap();
            p
        };
        let big = save(&picture(1600, 1200, 0), "big.png", "png", &[]);
        let small = save(&picture(1600, 1200, 0).scale_simple(400, 300, InterpType::Hyper).unwrap(), "small.jpg", "jpeg", &[("quality", "70")]);
        let other = save(&picture(1600, 1200, 3), "other.png", "png", &[]);
        let cropped = save(&picture(1600, 1200, 0).new_subpixbuf(0, 0, 1200, 1200), "square.png", "png", &[]);
        let fps: Vec<Fingerprint> = [&big, &small, &other, &cropped].iter().map(|p| fingerprint(p).unwrap()).collect();
        assert_eq!((fps[0].width, fps[0].height), (1600, 1200));
        assert_eq!((fps[1].width, fps[1].height), (400, 300));
        assert!(similar(&fps[0], &fps[1]), "same picture, smaller and as JPEG");
        assert!(!similar(&fps[0], &fps[2]), "another picture");
        assert!(!similar(&fps[0], &fps[3]), "another shape");
        assert_eq!(similar_groups(&fps), vec![vec![0, 1]]);
        let progress = AtomicUsize::new(0);
        let all = fingerprints(&[big.clone(), dir.path().join("not-a-picture.png")], &AtomicBool::new(false), &progress);
        assert!(all[0].is_some() && all[1].is_none());
        assert_eq!(progress.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn files_with_the_same_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir(d.join("sub")).unwrap();
        let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let mut big2 = big.clone();
        *big2.last_mut().unwrap() ^= 1; // same size and start, other end
        std::fs::write(d.join("a.bin"), &big).unwrap();
        std::fs::write(d.join("sub/a copy.bin"), &big).unwrap();
        std::fs::write(d.join("almost.bin"), &big2).unwrap();
        std::fs::write(d.join("x.txt"), "same").unwrap();
        std::fs::write(d.join("y.txt"), "same").unwrap();
        std::fs::write(d.join("z.txt"), "diff").unwrap();
        std::fs::write(d.join("empty1"), "").unwrap();
        std::fs::write(d.join("empty2"), "").unwrap();
        let files = walk(d, &AtomicBool::new(false));
        assert_eq!(files.len(), 8);
        let groups = exact_groups(&files, &AtomicBool::new(false), &AtomicUsize::new(0));
        // Stable separator for the expected groups; Windows displays backslashes.
        let rel_key = |rel: &std::path::Path| rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
        let names: Vec<Vec<String>> = groups.iter().map(|g| g.iter().map(|f| rel_key(f.path.strip_prefix(d).unwrap())).collect()).collect();
        assert_eq!(names, [vec!["a.bin", "sub/a copy.bin"], vec!["x.txt", "y.txt"]]);
        assert!(is_picture_name("IMG_1.JPG") && !is_picture_name("notes.txt"));
    }
}
