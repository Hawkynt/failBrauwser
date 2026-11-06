# failBrauwser on Windows

failBrauwser is one codebase that builds natively for Linux and Windows: the same GTK 3
widgets on both, with the platform-specific work kept in `src/win/` (Win32 and COM details),
`src/osprim.rs` (the syscall-level primitives both sides share) and paired
`*.linux.rs`/`*.windows.rs` modules where a whole IO strategy differs. Nothing in the UI
layer branches on the platform.

## Building and installing

```powershell
winpack\build.ps1            # debug build (app + archive helper)
winpack\build.ps1 -Release   # release build
winpack\package.ps1          # build and pack the release pair (zip and single-file exe)
winpack\install.ps1          # install the newest package for the current user
```

`winpack\bootstrap.ps1` fetches the pinned GTK stack (see `winpack\packages.txt`) from an
MSYS2 mirror into `winpack\sysroot`, and `winpack\genimports.ps1` creates the MSVC import
libraries for it; `winpack\env.ps1` points a shell at that sysroot. The toolchain needs
Visual Studio's C++ tools (link, dumpbin) and, for archives, the .NET SDK (NativeAOT — no
clang required).

### Cross-building from Linux

`winpack/cross.sh` builds the same program from Linux: the `x86_64-pc-windows-gnu` Rust
target with the MinGW-w64 cross GCC as linker, against the very sysroot `bootstrap.ps1`
pins (`cross.sh bootstrap` fetches the packages listed in `bootstrap-lock.txt`). Tests and
the package run under Wine:

```sh
winpack/cross.sh bootstrap        # once: the pinned GTK stack into winpack/sysroot
winpack/cross.sh build            # debug build
winpack/cross.sh test             # cargo test under Wine, on a private X server
winpack/cross.sh test-each        # every test alone, with a time limit (one hang hides nothing)
winpack/cross.sh package          # the zip and the single-file exe into winpack/out
```

The cross build embeds the manifest with `windres` instead of the MSVC linker's flags, and
ships the archive helper as a self-contained single-file .NET program (NativeAOT does not
compile across operating systems). PowerShell does not run under Wine, so the UI scenarios
are driven by the test binary itself there (`FB_UI_HARNESS=builtin`, the same contract as
`tests/ui/harness.ps1`). Wine is a stand-in, not Windows: the shell recycle bin, WNet,
WPD, the virtual disk service and PowerShell are missing or partial in it, and the tests
of exactly those parts need a real Windows (see *Test suite state* below).

A `failbrauwser.exe` straight out of `target` only runs from a shell that sourced
`winpack\env.ps1` — its GTK DLLs live in `winpack\sysroot\bin`, and double-clicking it
from Explorer (a clean PATH) fails with "libcairo-2.dll not found". The package, the
installed tree and the single-file exe below all carry the DLLs where the loader finds
them.

The packaged layout is `bin` (program, `fb-archive` helper, all DLLs), `lib` (gdk-pixbuf
loaders) and `share` (icons, fonts, schemas, licenses) side by side — GTK finds the loaders
relative to the binaries, which is why the folders keep that shape. `install.ps1` rewrites
the pixbuf loader cache after copying (the built-in one names the building machine's
folders), and adds a Start Menu entry.

`package.ps1` delivers that tree twice: as the zip (the updater's channel, and what
`install.ps1` unpacks) and as the single-file release
`failbrauwser-<version>-x86_64-windows.exe` — the same zip embedded in a small launcher
(`src/portable.rs`, its own crate `winpack\onefile` with a static CRT, so the launcher
itself imports only KERNEL32/ntdll: no installer, no VC++ runtime). On first run it
unpacks the tree into `%LOCALAPPDATA%\Programs\failbrauwser` (`FB_PORTABLE_PREFIX`
overrides), stamps `.payload-version` and re-unpacks only when the embedded version is
newer than the stamp, regenerates the loader cache for the new location, then starts the
program and passes its exit code through. Nothing is registered in the system, and no
DLLs or data ever sit next to the file itself.

## Updating

*Help → Check for Updates* on Windows installs the newest release's ready-built
`failbrauwser-<version>-x86_64-windows.zip` (the same name formula `package.ps1` produces).
The running program file is renamed aside — Windows allows renaming a running image, not
overwriting it — the fresh `bin`/`lib`/`share` contents are copied in, the old file deleted
and the new program started. The *build from source* channel is Linux-only there (it runs
`install.sh`); a copy installed machine-wide shows the manual route instead
(`winpack\install.ps1`).

## What is where

| Area | Linux | Windows |
|---|---|---|
| System calls under copy/delete/trash | `osprim` errno predicates | same file, Win32 codes through `raw_os_error` |
| Drives, mount state | UDisks2 / GVfs | `win::storage` (drive types, geometry, SSD/HDD), WNet for shares |
| Trash | freedesktop `$Trashdir` | Recycle Bin through `IFileOperation` (`win::recyclebin`) |
| Network folders | GVfs `smb://`; a typed `\\host\share` becomes its `smb://` address | real UNC paths via WNet — a typed `\\host\share` is opened as what it already is |
| Servers (SFTP/FTP) | system tools | in-app VFS (`src/remote_fs/`, russh/suppaftp) — browsable like archives, not mapped drives |
| Phones and cameras | GVfs MTP/gPhoto2 | WPD COM (`win::wpd`) |
| Disk images as drives | UDisks2 loop devices (raw file system images, ISO) | the shell's own Mount verb, else the virtual disk API (`win::vdisk`): ISO, IMG, VHD, VHDX |
| Searching below a server or phone folder | the GVfs mount is a folder on disk | the folders are listed over the app's own connection (`search::search_listed`) |
| Desktop links | `.desktop` files | `.lnk` (`win::shelllink`) |
| Owner/group columns | passwd | SID → name |
| Dark mode | org.gnome…ColorScheme | `AppsUseLightTheme` registry feed |
| Open Terminal Here | x-terminal-emulator | `wt.exe`, falling back to `cmd.exe` |
| File clipboard interop | clipboard URI lists | CF_HDROP with Explorer, both directions |
| Autostart | autostart `.desktop` | `HKCU\…\Run` value |

## Known limitations and differences

- **Recycle Bin “Empty Trash” button**: emptying the Windows bin empties the machine's bin,
  including your own older files — the UI offers per-item *Delete permanently* instead; the
  scenario only keeps the Linux-side button test.
- **Symlinks** need Developer Mode (or elevation); without it failBrauwser says so and falls
  back to copying. The `summary.fbt` fixture uses one, so that scenario skips on machines
  without it.
- **Recycle Bin quirks**: a permanent delete can leave a stub entry the shell keeps
  reporting; those orphans never appear in the bin view (filtered in `win::recyclebin`).
- **Archives whose entries differ only in case** (`a.txt` and `A.TXT` side by side) can
  be listed but not edit-rebuilt on default NTFS: the helper stages entries through the
  volume, and NTFS separates such names only when per-directory case sensitivity is
  enabled. `Remove_IsCaseSensitive` therefore answers inconclusive on Windows.
- **Copying**: NTFS alternate data streams are not carried by the raw copy loop, ACLs
  inherit rather than travel, and ReFS block-cloning is attempted where available. Copies
  to removable media are paced by flush windows (there is no `sync_file_range` twin).
- **Machine policy**: this project's updater hands PowerShell the swap script as a command,
  never as a file, because software-restriction policies can block script files below the
  temp folder. Same reason `install.ps1` lives in the repo.
- **Search on remote folders** lists folder by folder over the connection, so it is as
  fast as the server answers listings; it does not look inside archives on the server
  (each would be downloaded first) — the *Inside archives* switch is off there.
- **Encrypted drives**: unlocking LUKS sticks, and the LVM volumes inside them, is the
  Linux side's (UDisks2, device-mapper); Windows unlocks BitLocker drives itself, and
  failBrauwser lists them once Windows has.
- **Disk images**: Windows mounts ISO, IMG, VHD and VHDX itself; raw file system images
  Linux loop-mounts (`.ntfs`, `.ext4`, …) are not offered there. Without the shell's Mount
  verb, attaching needs administrator rights.
- **Serving and consuming the file chooser via XDG portal** is a Linux desktop feature;
  Windows has its own picker story and failBrauwser does not hook it.
- **Speed-chart background tint** comes from the window manager theme on Linux; on Windows
  the app palette is used.
- **Win32 GLib resolves the Desktop folder through the roaming profile**, so UI tests do
  not create desktop links on Windows (the `.lnk` writer itself has unit tests).
- Content types: without a system MIME database, the file-type icons fall back through
  Windows' registered extensions where GLib can read them.
- **Theme asset warning (unresolved)**: packaged runs print one
  `Gtk-WARNING: Could not load a pixbuf from /org/gtk/libgtk/theme/Adwaita/assets/bullet-symbolic.svg`
  at startup. Everything else loads (the loader cache names the right folders, scan mode
  behaves the same, and extra sysroot trees make no difference); the query tool inside the
  packaged `bin` renders that very svg fine, and repeated snapshot diffs could not decide
  whether any pixels change — failBrauwser has no theme-drawn checkable glyphs, so no
  visible impact was identified. Recorded rather than papered over.
- **Session bus**: win32 GLib prints `win32 session dbus binary not found` once (GTK for
  Windows ships `gdbus.exe` but no `dbus-daemon.exe`) — harmless: the uniqueness check
  still works over GLib's own local bus (probed during the GTK spike, and the packaged
  sysroot has never had a `dbus-daemon.exe`), and selftests additionally run
  `NON_UNIQUE`/`HANDLES_COMMAND_LINE` so they need no bus at all.

## Test suite state on Windows

`cargo test` runs everything; the `.fbt` UI suite runs through `tests/ui/harness.ps1`.
Scenarios that need Linux-only services skip with a printed reason:
`portal.fbt`, `portal_frontend.fbt` (XDG portal),
`desktop_links.fbt` (roaming-profile Desktop, see above), `summary.fbt` (symlink fixture),
and the portal/default-app parts elsewhere. Timing budgets for the big-listing paints are
widened for win32 GTK plus Defender (`big_listings.fbt`) — measured here, not guessed.
All 42 scenarios pass on this host as of this writing, run serially
(`RUST_TEST_THREADS=1 cargo test`).

Under Wine (`winpack/cross.sh test-each`, Wine 11) everything that does not need a
Windows service Wine lacks passes — 36 of the 43 UI scenarios among them. What fails there,
and why it says nothing about Windows:

| Wine lacks | Tests |
|---|---|
| the shell Recycle Bin (`IFileOperation` answers E_NOTIMPL) | `win::recyclebin::*`, the delete-to-trash steps of `trash`, `file_ops`, `search`, `find_duplicates`, `undo_operations` |
| the hidden file attribute (`attrib +h` reads back as R) | `readonly_and_hidden_travel_through_a_copy`, `copies_tree_with_attributes_and_times` |
| case-only renames (its own `ren a.txt A.txt` keeps `a.txt`) | `case_only_rename_keeps_one_file` |
| a network (WNet answers ERROR_NO_NETWORK), WPD devices | `the_local_machine_answers_a_share_probe`, `wpd::tests::the_worker_answers_a_device_enumeration` |
| PowerShell, `curl.exe`, `tar.exe` | the updater's swap test and `update.fbt`, `remove_is_exact_and_folders_go_completely` |
| cmd joining `"…".done` in a redirection | `custom_actions.fbt` |

`files_with_the_same_bytes` and `verify_option_reads_every_copy_back` hung in one full
Wine run (right after a recycle bin test had to be killed) and passed in every other.

The single-file launcher cannot be exercised under Wine with Xvfb either: GTK started
without a console of its own dies on an X error inside Wine's X11 driver there (Notepad in
the same setup does not), so the launcher is checked for unpacking, the version stamp and
`--version` only; the program it starts runs the UI scenarios directly.
