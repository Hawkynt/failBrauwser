//! What an application asks for when it wants the user to pick a file through the desktop
//! portal (`org.freedesktop.impl.portal.FileChooser`): open one or more files or a folder,
//! save one under a name, or save several into a folder; with filters by name or type.

use gtk::prelude::*;
use gtk::{gio, glib};
use std::path::PathBuf;

/// One entry of the "files of type" list: name patterns (`*.png`) and content types
/// (`image/*`). Without either it matches no file (a folder chooser shows folders only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub name: String,
    pub globs: Vec<String>,
    pub types: Vec<String>,
}

impl Filter {
    /// Whether a file with this name and content type is shown.
    pub fn matches(&self, name: &str, content_type: &str) -> bool {
        let lower = name.to_lowercase();
        self.globs.iter().any(|g| glob(&g.to_lowercase(), &lower))
            || self.types.iter().any(|t| {
                t == content_type || t.strip_suffix("/*").is_some_and(|major| content_type.split('/').next() == Some(major)) || gio::content_type_is_a(content_type, t)
            })
    }
}

/// Shell-style `*` and `?` matching.
fn glob(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Open { multiple: bool, directory: bool },
    /// Save one file, the name suggested.
    Save { name: String },
    /// Save these files into a folder chosen.
    SaveFiles { names: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub title: String,
    /// The text of the button (`_Open`, `_Save`, or the application's own).
    pub accept_label: Option<String>,
    pub mode: Mode,
    pub filters: Vec<Filter>,
    /// Index into `filters`.
    pub current_filter: Option<usize>,
    pub folder: Option<PathBuf>,
}

fn bytes_path(v: &glib::Variant) -> Option<PathBuf> {
    let b: Vec<u8> = v.fixed_array::<u8>().ok()?.to_vec();
    let b: Vec<u8> = b.into_iter().take_while(|c| *c != 0).collect();
    (!b.is_empty()).then(|| path_from_bytes(b))
}

#[cfg(unix)]
fn path_from_bytes(b: Vec<u8>) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(std::ffi::OsString::from_vec(b))
}

#[cfg(not(unix))]
fn path_from_bytes(b: Vec<u8>) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&b).into_owned())
}

fn filter_of(v: &glib::Variant) -> Option<Filter> {
    let name: String = v.child_value(0).get()?;
    let list = v.child_value(1);
    let (mut globs, mut types) = (Vec::new(), Vec::new());
    for i in 0..list.n_children() {
        let e = list.child_value(i);
        let (kind, pattern): (u32, String) = (e.child_value(0).get()?, e.child_value(1).get()?);
        if kind == 0 { globs.push(pattern) } else { types.push(pattern) }
    }
    Some(Filter { name, globs, types })
}

/// The filter as the portal writes it: `(sa(us))`.
pub fn filter_variant(f: &Filter) -> glib::Variant {
    let list: Vec<(u32, String)> = f.globs.iter().map(|g| (0u32, g.clone())).chain(f.types.iter().map(|t| (1u32, t.clone()))).collect();
    (f.name.clone(), list).to_variant()
}

/// Reads a portal call's title and options (`a{sv}`); `method` is OpenFile, SaveFile or
/// SaveFiles.
pub fn parse(method: &str, title: &str, options: &glib::Variant) -> Request {
    let dict = glib::VariantDict::new(Some(options));
    let get = |k: &str| dict.lookup_value(k, None);
    let flag = |k: &str| get(k).and_then(|v| v.get::<bool>()).unwrap_or(false);
    let mut filters: Vec<Filter> = get("filters").map(|v| (0..v.n_children()).filter_map(|i| filter_of(&v.child_value(i))).collect()).unwrap_or_default();
    let current = get("current_filter").and_then(|v| filter_of(&v));
    // The current filter is one of the list, or the only one when there is no list.
    let current_filter = match current {
        Some(c) => match filters.iter().position(|f| *f == c) {
            Some(i) => Some(i),
            None => {
                filters.insert(0, c);
                Some(0)
            }
        },
        None if filters.is_empty() => None,
        None => Some(0),
    };
    let current_file = get("current_file").and_then(|v| bytes_path(&v));
    let folder = get("current_folder").and_then(|v| bytes_path(&v)).or_else(|| current_file.as_ref().and_then(|f| f.parent().map(|p| p.to_path_buf())));
    let mode = match method {
        "SaveFile" => Mode::Save {
            name: get("current_name")
                .and_then(|v| v.get::<String>())
                .or_else(|| current_file.as_ref().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default(),
        },
        "SaveFiles" => Mode::SaveFiles {
            names: get("files").map(|v| (0..v.n_children()).filter_map(|i| bytes_path(&v.child_value(i))).map(|p| p.display().to_string()).collect()).unwrap_or_default(),
        },
        _ => Mode::Open { multiple: flag("multiple"), directory: flag("directory") },
    };
    Request { title: title.to_string(), accept_label: get("accept_label").and_then(|v| v.get::<String>()), mode, filters, current_filter, folder }
}

/// The answer to the portal: `(u response, a{sv} results)`; `None`: cancelled.
pub fn reply(chosen: Option<(&[PathBuf], Option<&Filter>)>) -> glib::Variant {
    let results = glib::VariantDict::new(None);
    let code = match chosen {
        Some((paths, filter)) => {
            let uris: Vec<String> = paths.iter().map(|p| gio::File::for_path(p).uri().to_string()).collect();
            results.insert_value("uris", &uris.to_variant());
            if let Some(f) = filter {
                results.insert_value("current_filter", &filter_variant(f));
            }
            0u32
        }
        None => 1u32,
    };
    // Not (code, dict).to_variant(): that boxes the dictionary as a "v".
    glib::Variant::tuple_from_iter([code.to_variant(), results.end()])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(text: &str) -> glib::Variant {
        glib::Variant::parse(Some(glib::VariantTy::new("a{sv}").unwrap()), text).unwrap()
    }

    #[test]
    fn open_requests() {
        let o = opts("{'multiple': <true>, 'accept_label': <'_Import'>, 'filters': <[('Pictures', [(uint32 0, '*.png'), (uint32 1, 'image/jpeg')]), ('All files', [(uint32 0, '*')])]>, 'current_folder': <b'/home/me/Pictures'>}");
        let r = parse("OpenFile", "Import pictures", &o);
        assert_eq!(r.mode, Mode::Open { multiple: true, directory: false });
        assert_eq!(r.accept_label.as_deref(), Some("_Import"));
        assert_eq!(r.filters.len(), 2);
        assert_eq!(r.current_filter, Some(0));
        assert_eq!(r.folder, Some(PathBuf::from("/home/me/Pictures")));
        let pics = &r.filters[0];
        assert!(pics.matches("Beach.PNG", "image/png"));
        assert!(pics.matches("photo", "image/jpeg"));
        assert!(!pics.matches("notes.txt", "text/plain"));
        assert!(r.filters[1].matches("anything.bin", "application/octet-stream"));
        let r = parse("OpenFile", "Folder", &opts("{'directory': <true>}"));
        assert_eq!(r.mode, Mode::Open { multiple: false, directory: true });
        assert!(r.filters.is_empty() && r.current_filter.is_none());
        let images = Filter { name: "Images".into(), globs: vec![], types: vec!["image/*".into()] };
        assert!(images.matches("x.webp", "image/webp") && !images.matches("x.txt", "text/plain"));
    }

    #[test]
    fn save_requests_and_answers() {
        let o = opts("{'current_name': <'Report 2026.pdf'>, 'current_filter': <('PDF', [(uint32 0, '*.pdf')])>}");
        let r = parse("SaveFile", "Save report", &o);
        assert_eq!(r.mode, Mode::Save { name: "Report 2026.pdf".into() });
        assert_eq!((r.filters.len(), r.current_filter), (1, Some(0)));
        // Paths come as NUL-terminated bytes.
        let d = glib::VariantDict::new(None);
        d.insert_value("current_file", &glib::Variant::array_from_fixed_array(b"/home/me/old.txt\0"));
        let r = parse("SaveFile", "Save", &d.end());
        assert_eq!(r.mode, Mode::Save { name: "old.txt".into() });
        assert_eq!(r.folder, Some(PathBuf::from("/home/me")));
        let r = parse("SaveFiles", "Save all", &opts("{'files': <[b'a.png', b'b.png']>}"));
        assert_eq!(r.mode, Mode::SaveFiles { names: vec!["a.png".into(), "b.png".into()] });

        let answer = reply(Some((&[PathBuf::from("/tmp/a b.txt")], Some(&r.filters.first().cloned().unwrap_or(Filter { name: "Text".into(), globs: vec!["*.txt".into()], types: vec![] })))));
        assert_eq!(answer.type_().as_str(), "(ua{sv})");
        assert_eq!(answer.child_value(0).get::<u32>(), Some(0));
        let dict = glib::VariantDict::new(Some(&answer.child_value(1)));
        assert_eq!(dict.lookup_value("uris", None).unwrap().get::<Vec<String>>().unwrap(), ["file:///tmp/a%20b.txt"]);
        assert!(dict.lookup_value("current_filter", None).is_some());
        assert_eq!(reply(None).child_value(0).get::<u32>(), Some(1));
        assert!(glob("*.tar.gz", "x.tar.gz") && glob("img_??.jpg", "img_01.jpg") && !glob("*.png", "a.jpg"));
    }
}
