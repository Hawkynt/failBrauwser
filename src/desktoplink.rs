//! Links on the desktop: on Linux a freedesktop.org Desktop Entry of type `Link` (a
//! `.desktop` file with `Type=Link` and `URL=`), which GNOME, KDE Plasma, Xfce, Cinnamon,
//! MATE, LXQt and most desktop icon programs show as an icon that opens the folder, drive
//! or file it points to. See <https://specifications.freedesktop.org/desktop-entry-spec/latest/>.
//! On Windows the same links are `.lnk` shell links (see [`crate::win::shelllink`]), the
//! only kind Explorer understands; a themed icon name has no meaning to Explorer, so the
//! link shows the target's own icon — usually the better picture anyway.

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

/// A file name for the link: the name without characters the filesystem refuses, with `ext`;
/// one already there gets a number ("Music (2).desktop").
fn free_file_name(dir: &Path, name: &str, ext: &str) -> PathBuf {
    #[cfg(windows)]
    let bad: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
    #[cfg(not(windows))]
    let bad: &[char] = &[];
    let mut base: String = name.chars().map(|c| if bad.contains(&c) || c == '/' || c == '\0' { '_' } else { c }).collect();
    // Windows also refuses a name ending in a dot or a space.
    #[cfg(windows)]
    {
        base = base.trim_end_matches(['.', ' ']).to_string();
    }
    let base_trimmed = base.trim().trim_start_matches('.').to_string();
    base = if base_trimmed.is_empty() { "Link".into() } else { base_trimmed };
    let mut path = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while path.symlink_metadata().is_ok() {
        path = dir.join(format!("{base} ({n}).{ext}"));
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
    let path = free_file_name(dir, name, "desktop");
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

/// What a link points at: an address (`file:///…`, `trash:///`, a device address) or a
/// program to start with its arguments (for pages only failBrauwser can show).
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Url(String),
    Command(Vec<String>),
}

/// A command without arguments (the degenerate case, kept out of the arms).
const NO_ARGS: &[String] = &[];

/// Places one link on the desktop, named `name`, showing `icon` (a themed icon name,
/// used by the Linux entry; Explorer takes the target's own picture on Windows).
pub fn place(name: &str, icon: &str, target: &Target) -> io::Result<PathBuf> {
    place_in(&desktop_dir(), name, icon, target)
}

/// Places one link in `dir` (made if missing); gives the file made.
pub fn place_in(dir: &Path, name: &str, icon: &str, target: &Target) -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        // A themed icon name means nothing to Explorer: the link shows the target's own picture.
        let _ = icon;
        return win_place_in(dir, name, target);
    }
    #[cfg(not(windows))]
    {
        let text = match target {
            Target::Url(url) => entry_text(name, url, icon),
            Target::Command(command) => app_entry_text(name, &command.iter().map(String::as_str).collect::<Vec<_>>(), icon),
        };
        write_in(dir, name, &text)
    }
}

#[cfg(windows)]
fn win_place_in(dir: &Path, name: &str, target: &Target) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = free_file_name(dir, name, "lnk");
    let (me, file, exe, args); // owners outliving the borrowed view the spec carries
    let spec = match target {
        // A program link: first word the program, the rest its command line.
        Target::Command(command) => {
            let (program, rest) = match command.split_first() {
                Some((program, rest)) => (program.as_str(), rest),
                None => ("", NO_ARGS),
            };
            exe = program.to_string();
            args = rest.iter().map(|a| win_quote(a)).collect::<Vec<_>>().join(" ");
            crate::win::shelllink::Target::Path { target: std::path::Path::new(&exe), arguments: &args }
        }
        // An address the shell can name becomes a plain path link; the trash is a shell
        // folder, not a path; anything else (a `wpd://…` device address) only failBrauwser
        // can open, so the link starts this very program with the address.
        Target::Url(url) if url == crate::location::TRASH_URI => crate::win::shelllink::Target::RecycleBin,
        Target::Url(url) => match glib::filename_from_uri(url) {
            Ok((f, _)) => {
                file = f;
                crate::win::shelllink::Target::Path { target: &file, arguments: "" }
            }
            Err(_) => {
                me = std::env::current_exe()?;
                crate::win::shelllink::Target::Path { target: &me, arguments: url }
            }
        },
    };
    crate::win::shelllink::create(&path, &spec)?;
    Ok(path)
}

/// Quotes one command-line word for the quoted-argument rules of the receiving program.
#[cfg(windows)]
fn win_quote(a: &str) -> String {
    if a.chars().any(char::is_whitespace) || a.contains('"') { format!("\"{}\"", a.replace('"', "\\\"")) } else { a.to_string() }
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

    #[cfg(windows)]
    #[test]
    fn the_desktop_folder_glib_names_is_a_real_one() {
        let d = desktop_dir();
        assert!(d.is_dir(), "GLib names the Explorer desktop (redirected or not): {}", d.display());
    }

    #[cfg(windows)]
    #[test]
    fn windows_links_open_what_the_shell_resolves_them_to() {
        let dir = tempfile::tempdir().unwrap();
        let exe = std::env::current_exe().unwrap();
        let a = place_in(dir.path(), "Pages", "drive-harddisk", &Target::Command(vec![exe.display().to_string(), "drives:///".into()])).unwrap();
        assert_eq!(a, dir.path().join("Pages.lnk"), "the file carries the .lnk ending Explorer opens");
        let (path, args) = crate::win::shelllink::read(&a).unwrap();
        assert_eq!(path, exe.to_string_lossy(), "the link starts this very program");
        assert_eq!(args, "drives:///", "the page argument survives the binary round-trip");

        let folder = dir.path().join("My Music");
        std::fs::create_dir(&folder).unwrap();
        let b = place_in(dir.path(), "My Music", "folder", &Target::Url(file_url(&folder).unwrap())).unwrap();
        assert_eq!(std::path::Path::new(&crate::win::shelllink::read(&b).unwrap().0), folder, "a file address becomes a path link");
        let c = place_in(dir.path(), "My Music", "folder", &Target::Url(file_url(&folder).unwrap())).unwrap();
        assert_eq!(c, dir.path().join("My Music (2).lnk"), "a second link with the same name gets a number");

        let d = place_in(dir.path(), "Bin", "user-trash", &Target::Url("trash:///".into())).unwrap();
        assert!(crate::win::shelllink::read(&d).is_ok(), "the trash address becomes a loadable bin link (its id list is proven in win::shelllink)");

        let e = place_in(dir.path(), "a*b:c", "folder", &Target::Command(vec![exe.display().to_string()])).unwrap();
        assert_eq!(e, dir.path().join("a_b_c.lnk"), "characters the filesystem refuses become underscores");
    }
}
