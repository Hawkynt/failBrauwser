mod ui;

const HELP: &str = "\
Usage: failbrauwser [OPTION…] [FOLDER|ARCHIVE…]

Opens a window for each location (or your home folder). A running failBrauwser
opens the window instead of starting again.

Options:
  -d, --daemon    start in the background without a window (for autostart)
  -q, --quit      quit the running instance, including a background one
  -h, --help      show this help
  -V, --version   show the version
";

fn main() -> gtk::glib::ExitCode {
    // Windows keeps autostart in the per-user Run key (Linux ships an XDG autostart file);
    // managed from here so no installer magic is needed, and answered without opening GTK.
    #[cfg(windows)]
    if let Some(on) = match std::env::args().nth(1).unwrap_or_default().as_str() {
        "--autostart-on" => Some(true),
        "--autostart-off" => Some(false),
        _ => None,
    } {
        let outcome = if on {
            let exe = std::env::current_exe().map(|p| format!("\"{}\" --daemon", p.display())).map_err(std::io::Error::other);
            match exe {
                Ok(cmd) => failbrauwser::win::reg::set_autostart(&cmd),
                Err(e) => Err(e),
            }
        } else {
            failbrauwser::win::reg::clear_autostart()
        };
        let ok = match outcome {
            Ok(()) => {
                println!("{}", if on { "failBrauwser will start when you sign in." } else { "Autostart removed." });
                true
            }
            Err(e) => {
                eprintln!("Cannot change autostart: {e}");
                false
            }
        };
        return if ok { gtk::glib::ExitCode::SUCCESS } else { gtk::glib::ExitCode::FAILURE };
    }
    // Answered without GTK or D-Bus: never opens a window.
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                #[cfg(windows)]
                print!("  --autostart-on / --autostart-off\n                  start failBrauwser at sign-in, or stop it (HKCU Run key)\n");
                return gtk::glib::ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("failbrauwser {}", env!("CARGO_PKG_VERSION"));
                return gtk::glib::ExitCode::SUCCESS;
            }
            _ => {}
        }
    }
    #[cfg(unix)]
    raise_open_file_limit();
    ui::app::run()
}

/// Lifts the soft limit on open files (often 1024) to the hard limit: GLib aborts the whole
/// process when it finds no free descriptor, which must not end a long-running instance.
/// Windows grows its handle table on demand, so it has no such limit to lift.
#[cfg(unix)]
fn raise_open_file_limit() {
    let mut l = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    unsafe {
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut l) == 0 && l.rlim_cur < l.rlim_max {
            l.rlim_cur = l.rlim_max;
            libc::setrlimit(libc::RLIMIT_NOFILE, &l);
        }
    }
}
