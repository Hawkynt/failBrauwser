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
    // Answered without GTK or D-Bus: never opens a window.
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return gtk::glib::ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("failbrauwser {}", env!("CARGO_PKG_VERSION"));
                return gtk::glib::ExitCode::SUCCESS;
            }
            _ => {}
        }
    }
    raise_open_file_limit();
    ui::app::run()
}

/// Lifts the soft limit on open files (often 1024) to the hard limit: GLib aborts the whole
/// process when it finds no free descriptor, which must not end a long-running instance.
fn raise_open_file_limit() {
    let mut l = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    unsafe {
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut l) == 0 && l.rlim_cur < l.rlim_max {
            l.rlim_cur = l.rlim_max;
            libc::setrlimit(libc::RLIMIT_NOFILE, &l);
        }
    }
}
