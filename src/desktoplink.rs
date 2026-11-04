//! Links on the desktop: a freedesktop.org Desktop Entry of type `Link` (a `.desktop` file
//! with `Type=Link` and `URL=`), which GNOME, KDE Plasma, Xfce, Cinnamon, MATE, LXQt and
//! most desktop icon programs show as an icon that opens the folder, drive or file it
//! points to. See <https://specifications.freedesktop.org/desktop-entry-spec/latest/>.

use gtk::glib;
use std::io;
use std::path::{Path, PathBuf};

/// The desktop folder (`XDG_DESKTOP_DIR`, usually `~/Desktop`).
pub fn desktop_dir() -> PathBuf {
    glib::user_special_dir(glib::UserDirectory::Desktop).unwrap_or_else(|| glib::home_dir().join("Desktop"))
}

/// The text of a link entry. `url` is an address (`file:///home/me/Music`, `mtp://…/`,
/// `trash:///`), `icon` a themed icon name.
pub fn entry_text(name: &str, url: &str, icon: &str) -> String {
    format!("[Desktop Entry]\nVersion=1.0\nType=Link\nName={}\nURL={}\nIcon={}\n", escape(name), escape(url), escape(icon))
}

/// The text of an entry that starts a program instead: `command` is the program and its
/// arguments (`failbrauwser drives:///` for a page that only failBrauwser can show).
pub fn app_entry_text(name: &str, command: &[&str], icon: &str) -> String {
    let exec: Vec<String> = command.iter().map(|a| quote_arg(a)).collect();
    format!("[Desktop Entry]\nVersion=1.0\nType=Application\nName={}\nExec={}\nIcon={}\nTerminal=false\n", escape(name), escape(&exec.join(" ")), escape(icon))
}

/// An argument of `Exec=`: quoted when it holds spaces or characters the spec reserves,
/// with `"`, `` ` ``, `$` and `\` escaped inside the quotes; a `%` is doubled.
fn quote_arg(a: &str) -> String {
    let a = a.replace('%', "%%");
    if !a.is_empty() && !a.chars().any(|c| c.is_whitespace() || "\"'\\><~|&;$*?#()`".contains(c)) {
        return a;
    }
    let mut out = String::from("\"");
    for c in a.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Values are one line: backslashes and line breaks are written as escapes.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

/// The address of a local file or folder.
pub fn file_url(path: &Path) -> Option<String> {
    glib::filename_to_uri(path, None).ok().map(|u| u.to_string())
}

/// A file name for the link: the name without slashes, with `.desktop`; one already there
/// gets a number ("Music (2).desktop").
fn free_file_name(dir: &Path, name: &str) -> PathBuf {
    let mut base: String = name.chars().map(|c| if c == '/' || c == '\0' { '_' } else { c }).collect();
    let base_trimmed = base.trim().trim_start_matches('.').to_string();
    base = if base_trimmed.is_empty() { "Link".into() } else { base_trimmed };
    let mut path = dir.join(format!("{base}.desktop"));
    let mut n = 2;
    while path.symlink_metadata().is_ok() {
        path = dir.join(format!("{base} ({n}).desktop"));
        n += 1;
    }
    path
}

/// Writes a link into `dir` (made if missing) and marks it executable, which KDE, Xfce and
/// GNOME's desktop icons ask for before they trust a `.desktop` file. Gives the file made.
pub fn create_in(dir: &Path, name: &str, url: &str, icon: &str) -> io::Result<PathBuf> {
    write_in(dir, name, &entry_text(name, url, icon))
}

/// Writes an entry's text into `dir` under a free name, executable.
pub fn write_in(dir: &Path, name: &str, text: &str) -> io::Result<PathBuf> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    std::fs::create_dir_all(dir)?;
    let path = free_file_name(dir, name);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    opts.mode(0o755);
    let mut f = opts.open(&path)?;
    f.write_all(text.as_bytes())?;
    // The umask may have taken the executable bits away.
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

/// Writes an entry on the desktop.
pub fn write(name: &str, text: &str) -> io::Result<PathBuf> {
    write_in(&desktop_dir(), name, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn entries_follow_the_spec() {
        let t = entry_text("My\nMusic", "file:///home/me/My%20Music", "folder-music");
        assert_eq!(t, "[Desktop Entry]\nVersion=1.0\nType=Link\nName=My\\nMusic\nURL=file:///home/me/My%20Music\nIcon=folder-music\n");
        let kf = glib::KeyFile::new();
        kf.load_from_data(&t, glib::KeyFileFlags::NONE).unwrap();
        assert_eq!(kf.string("Desktop Entry", "Name").unwrap(), "My\nMusic");
        assert_eq!(kf.string("Desktop Entry", "Type").unwrap(), "Link");
        assert_eq!(file_url(Path::new("/home/me/My Music")).as_deref(), Some("file:///home/me/My%20Music"));
    }

    #[test]
    fn program_entries_quote_their_arguments() {
        let t = app_entry_text("Drives", &["/opt/fail brauwser/bin/failbrauwser", "drives:///"], "drive-harddisk");
        assert_eq!(t, "[Desktop Entry]\nVersion=1.0\nType=Application\nName=Drives\nExec=\"/opt/fail brauwser/bin/failbrauwser\" drives:///\nIcon=drive-harddisk\nTerminal=false\n");
        assert_eq!(quote_arg("50%"), "50%%");
        assert_eq!(quote_arg("a$b\\c"), "\"a\\$b\\\\c\"");
        // The key file layer doubles the backslashes once more.
        let kf = glib::KeyFile::new();
        kf.load_from_data(&app_entry_text("x", &["/a\\b"], "i"), glib::KeyFileFlags::NONE).unwrap();
        assert_eq!(kf.string("Desktop Entry", "Exec").unwrap(), "\"/a\\\\b\"");
    }

    #[test]
    fn links_are_written_once_per_name() {
        let dir = tempfile::tempdir().unwrap();
        let desk = dir.path().join("Desktop");
        let a = create_in(&desk, "Music", "file:///m", "folder").unwrap();
        let b = create_in(&desk, "Music", "file:///m", "folder").unwrap();
        let c = create_in(&desk, "a/b", "file:///ab", "folder").unwrap();
        let d = create_in(&desk, ".", "file:///", "folder").unwrap();
        assert_eq!(a, desk.join("Music.desktop"));
        assert_eq!(b, desk.join("Music (2).desktop"));
        assert_eq!(c, desk.join("a_b.desktop"));
        assert_eq!(d, desk.join("Link.desktop"));
        #[cfg(unix)] // desktop environments demand the executable bit on .desktop files; Windows has none
        assert_eq!(std::fs::metadata(&a).unwrap().permissions().mode() & 0o777, 0o755);
        assert!(std::fs::read_to_string(&c).unwrap().contains("URL=file:///ab\n"));
    }
}
