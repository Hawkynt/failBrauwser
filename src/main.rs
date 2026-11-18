// Linked as a windowed program: starting this file — double-click, Start Menu entry, or the
// single-file launcher — opens failBrauwser's window and nothing else, no console. Started from
// a terminal it still answers there. Everything console-shaped it starts goes through
// `win::console::no_window`, which has no console of its own to share with them.
#![cfg_attr(windows, windows_subsystem = "windows")]

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
    // Before anything is started: the streams this program was given stay its own, so no tool it
    // runs holds a second copy of them (win\console.rs).
    #[cfg(windows)]
    failbrauwser::win::console::keep_our_handles_from_children();
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
    // Backup Mode and Volume Mountpoints, as the administrator through pkexec (links/helper.rs).
    let os: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if os.get(1).is_some_and(|a| a == failbrauwser::links::helper::ARG) {
        std::process::exit(failbrauwser::links::helper::main(&os[2..]));
    }
    // Opens the LVM volumes of an encrypted drive, as root through pkexec (see lvm.rs).
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--lvm") {
        std::process::exit(failbrauwser::lvm::helper_main(&args[2..]));
    }
    // A volume's name, its format, or the four files of its device icon, as the administrator
    // (fsformat::act): this same file started again through the shell's `runas` verb, and told by --outcome
    // which file to write its answer into. No window, no settings and no reading client belongs to that run
    // (docs\WINDOWS.md), and the flags it answers are exactly the ones the act module names.
    if failbrauwser::fsformat::act::is_act(args.get(1).map(String::as_str)) {
        std::process::exit(failbrauwser::fsformat::act::execute(&args[1..]));
    }
    // Started by git or ssh to ask for a password while a Git command of ours runs.
    if let Some(code) = ui::vcs::askpass::main_if_asked() {
        std::process::exit(code);
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
    let code = ui::app::run();
    // A client of a read that was still in flight when this run ended must not keep running behind
    // it: an invisible tool at no CPU, holding a pipe nobody reads (docs\TOOL-STARTS.md).
    failbrauwser::vcs::run::end_our_clients();
    code
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
