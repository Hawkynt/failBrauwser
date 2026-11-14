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
clang required). The program icon needs the Windows SDK's resource compiler: the build
scripts pick the newest `rc.exe` under `Windows Kits\10\bin\*\x64`, and a machine without
one builds the same programs with no icon, saying so at build time rather than failing.

The packaged layout is `bin` (program, `fb-archive` helper, all DLLs), `lib` (gdk-pixbuf
loaders) and `share` (icons, fonts, schemas, licenses) side by side — GTK finds the loaders
relative to the binaries, which is why the folders keep that shape. `install.ps1` rewrites
the pixbuf loader cache after copying (the built-in one names the building machine's
folders), and adds a Start Menu entry.

Both program files carry the application icon as a Win32 icon resource:
`data\icons\failbrauwser.ico` — the sizes 16, 24, 32, 48, 64, 128 and 256 rendered from
`data\icons\org.failbrauwser.FailBrauwser.svg` by `winpack\mkico.ps1` with the sysroot's
`rsvg-convert` — is compiled by `rc.exe` and handed to the linker by `build.rs` and
`winpack\onefile\build.rs`. That resource is what Explorer, the Start Menu entry
(`install.ps1` points its `IconLocation` at the program file) and the window titlebar show.
`package.ps1` additionally stages the svg into
`share\icons\hicolor\scalable\apps\org.failbrauwser.FailBrauwser.svg`, where GTK's themed
lookup finds it under the application id, so the window icon does not depend on the shell
falling back to the program file.

`package.ps1` delivers that tree twice: as the zip (the updater's channel, and what
`install.ps1` unpacks) and as the single-file release
`failbrauwser-<version>-x86_64-windows.exe` — the same zip embedded in a small launcher
(`src/portable.rs`, its own crate `winpack\onefile` with a static CRT, so the launcher
itself imports only KERNEL32/ntdll: no installer, no VC++ runtime). On first run it
unpacks the tree into `%LOCALAPPDATA%\Programs\failbrauwser` (`FB_PORTABLE_PREFIX`
overrides), stamps `.payload-version` with the payload's version *and* its build id — the
`payload-id.txt` that `package.ps1` writes into the tree — and re-unpacks whenever the
payload it carries is not the tree that is installed: an older installed version, or the
same version with an older or missing build id, is replaced, while a payload older than the
installed one leaves it alone. A rebuild at the same version therefore reaches an installed
copy, which a version comparison alone never does. `install.ps1` records the same id from
the package, so a manually installed tree is known to the launcher rather than read as
unknown. The loader cache is regenerated for the new location, then the program starts and
its exit code passes through. Nothing is registered in the system, and no DLLs or data ever
sit next to the file itself.

That unpack is all-or-nothing. The payload is staged in a folder inside the install folder and
its files are moved into place only once every entry has been read, inflated and CRC-verified,
so a payload that cannot be applied — a file that stands where one belongs, an entry that fails
its check — leaves the installed tree exactly as it was instead of replacing part of it, and the
staging folder is taken away again; `.payload-version` is written only after a complete unpack.
A launcher that cannot install says so where the person who started it will see it: a message
box naming the install folder and the reason when it was started with no console, and that same
text in the terminal when a terminal started it; either way it exits 1 rather than pretending to
have started. One boundary stays: a refresh needs every instance running from that folder closed
first. Renaming a running program file aside is possible — that is how *Help → Check for Updates*
swaps one from inside the app — and a launcher build that did it was measured here replacing all
2,882 files of a folder under a running instance, starting a second instance and leaving its staging
folder behind. The launcher therefore refuses instead: every file the payload would replace is tried
for writability before a byte is staged, so a folder something is using is answered with nothing
written to it. An in-app update (*Help → Check for Updates*) replaces the
tree's contents and leaves that stamp text as the launcher wrote it, so the launcher keeps
comparing against the payload it carried — for that launcher, an updated tree is not newer
than its own payload, so it stays untouched.

Neither program file opens a console window. `bin\failbrauwser.exe` is linked as a windowed
program (`#![cfg_attr(windows, windows_subsystem = "windows")]` in `src\main.rs`), so Explorer,
the Start Menu entry and the launcher show only failBrauwser's own window — the black console
that the CUI subsystem used to allocate for it is gone. Started from a terminal it still
answers there, because the terminal's console and its three standard handles are inherited —
one nuance of a windowed file: `cmd` and PowerShell do not wait for it, so the prompt returns
while the window is still open. Started with no console at all, it needs no handle fix: the C
runtime already backs those three descriptors for a console-less process, so a message the
program writes is dropped rather than failing the write. Measured here on 2026-11-12: a
windowed program started with nothing attached printed on both streams and stayed alive; the
same program built without any handle guard also survived a start through the shell. No
`NUL`-redirecting startup code is therefore part of the release. `fb-archive.exe` is
started with `CREATE_NO_WINDOW` and watches the process id the program hands it as `--parent`:
it quits within a second once that instance is gone, request running or not, and while the
instance lives it stops 45 s after its last request.

The mouse's back and forward keys are read by the program itself, not by GTK. Measured here on
2026-11-13 by posting messages to a realized GTK toplevel: `WM_XBUTTONDOWN` (`0x020B`, `winuser.h`)
does arrive as a GDK button press, but as **button 5 whichever of the two keys it was** — GDK's
win32 backend never looks at the high word of `wParam`, which is where Windows keeps
`XBUTTON1` and `XBUTTON2` apart, and GDK's button 5 is its wheel-down button. So each of the two
keys used to reach a folder view as a wheel click: nothing navigated. `src\win\xbuttons.rs` chains a
window procedure in front of GDK's on each of the program's own windows (the handle comes from GDK's
win32-only `gdk_win32_window_get_handle`), reads which key the message names, activates that
window's `back` or `forward` action, and keeps the X-button messages from going on to GTK — which
also leaves no phantom wheel click behind. The pane's `8`/`9` mapping in `src\ui\pane.rs` stays for
X11, where GDK numbers those keys that way and no such messages exist.

Measured with the key pair posted **from another process** into a running instance: the tab's
location moved one step back for `XBUTTON1` and one step forward for `XBUTTON2`, while the same
script against the build of 2026-11-12 — the same code except for that window procedure — kept the
location exactly where it had been. `tests\ui\mouse_keys.fbt` drives the same two messages from
inside the self-test, past both ends of the history included; its `!win` lines keep it out of a
Linux run, where the keys come through GDK instead. Not measured here: the input stack's own step
from a physical click to that message. Injected input does not land on this machine's desktop
session (a `SendInput` cursor move reports success and the cursor ends up elsewhere), so the
messages were posted rather than clicked; the pair and its `wParam` are Windows' documented shape
for those keys.

To start the plain `target\release\failbrauwser.exe` without any PATH setup, run
`winpack\deps.ps1 -Exe target\release\failbrauwser.exe -Flat` once after building: it
copies the DLL chain beside the program and the loaders and data trees inside its own
folder — where GTK derives them when a loaded DLL's folder carries no `bin` name,
unlike the packaged layout's `..\lib` — and rewrites the loader cache on the spot.
The folder stays build output; cargo ignores it.

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
| Network folders | GVfs `smb://` | real UNC paths via WNet — the whole engine works on them |
| Servers (SFTP/FTP) | system tools | in-app VFS (`src/remote_fs/`, russh/suppaftp) — browsable like archives, not mapped drives |
| Phones and cameras | GVfs MTP/gPhoto2 | WPD COM (`win::wpd`) |
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
- **Mouse back/forward in auxiliary windows**: the window procedure that reads those keys is
  chained onto failBrauwser's own file windows, because that is where a tab's history exists to
  navigate. Compare, Find Duplicates, Properties and the message dialogs are plain GTK windows
  with no history of their own, so a press there still reaches GTK as its wheel click.
- **Search on remote folders** (SFTP/FTP VFS) does not crawl yet.
- **Loop-mounted disk images**: Linux shows them through UDisks; on Windows they stay plain
  files (a virtual-CD story of its own).
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
`portal.fbt`, `portal_frontend.fbt` (XDG portal), `mount_image.fbt` (UDisks loop),
`desktop_links.fbt` (roaming-profile Desktop, see above), `summary.fbt` (symlink fixture),
and the portal/default-app parts elsewhere. Timing budgets for the big-listing paints are
widened for win32 GTK plus Defender (`big_listings.fbt`) — measured here, not guessed.
That makes 45 scripts across 46 scenario tests; run serially (`RUST_TEST_THREADS=1
cargo test --release --test ui_selftest`), 43 of them answered green on this host on
2026-11-13 in 376.95 s, the suite's slowest run here, with `big_listings.fbt` inside both of
its budgets that time. On 2026-11-12 the same command read 42 green of 45 in the morning
(217 s), 41 in each of three re-runs later that day (252 s, 288 s and 284 s) and 42 again in a
fourth (229.52 s, `cargo test --release --no-fail-fast`) — the extra one in the 41s being
`big_listings.fbt`, whose win paint budgets sit close to what this machine measures; see below. Three fail in every run: two sit with where this checkout lives rather
than with the archive work — the work folder lies under a path with a space
(`A:\Working Copies`), so `update.fbt` meets `curl: (3)` — the updater passes its
`file://` fixtures unescaped, and the fixture URL carries that same space — and
`custom_actions.fbt` loses its `%f`-expanded marker command to the same space (that very
scenario answers green when driven through a space-free work folder; harness.ps1 takes one,
re-checked 2026-11-10). One is the drives page: `lvm_drive.fbt` now reaches unlock, listing
and remount on Windows and answers green, while `encrypted_drive_with_lvm_inside` still
waits for a side-drive row to show the mount path the way its script compares it.

The six that failed on 2026-11-11 as one clipboard group — `file_ops.fbt`, `archives.fbt`,
`undo.fbt`, `queue.fbt`, `share_refresh.fbt` and `checksums.fbt`, each of them waiting on a
copy/paste job to land inside a script step — answered green again on 2026-11-12 with
nothing on that path changed in this checkout between the two measurements. Both records
stay here because the difference belongs to this machine's clipboard, not to the tree.
`open_as.fbt` pins the *Open as* menu, the passphrase prompt that comes before a chosen
volume is read, and the views the chosen kinds produce; what each kind reads out byte for
byte belongs to `archive_helper.rs` and `archive_vfs.rs`, where reading does not wait for a
clipboard job. `trash.fbt` once timed out while the key-derivation tests held every core;
run alone it passes. `big_listings.fbt` is the load-sensitive one rather than a defect:
measured over 2026-11-12 it stayed inside both win budgets in one run and exceeded them in
others — 15,963 ms against the 12,000 ms budget, then 13,084 ms (line 18) and 3,707 ms
(line 10) in two suite runs, then 3,707 ms and 16,665 ms in two runs of that scenario
alone, where an earlier run the same morning had answered green inside 25 s. The 16,665 ms
run linked no icon resource at all (`data\icons\failbrauwser.rc` taken away from the
build), and the 3,707 ms run carried it, so the stall does not come from the picture the
shell reads: it comes from this machine's disk and Defender state while 15,000 fresh rows
paint. The 284 s run failed it in yet another shape: no paint budget exceeded (longest
freeze 2,626 ms) and `wait-count 15150` simply outlived its step while the rows filled.
`thumbs::tests::generates_stores_and_reloads` is the same kind of miss on 2026-11-13: inside a
run of all 184 lib scenarios at once, the large flavor it had just generated was not the one
`load` answered a line later (`src\thumbs.rs:216`) — one of two reads of a png that `save` had
renamed moments before, on a machine reading it while Defender did too. Five runs of that
scenario alone and three runs of the whole lib binary that morning answered green.
Everything else `cargo test` runs answers green here: on 2026-11-12 everything short of the UI
suite read 238 passed, 0 failed and 2 ignored, the four `tests\app_icon.rs`, two
`tests\win_startup.rs` and two `tests\helper_lifetime.rs` scenarios among them. Re-run on
2026-11-13 over the same targets, doc-tests apart — that target carries no scenario — it read
241 passed, 1 failed and 2 ignored in one `--no-fail-fast` run, that thumbnail scenario the one
failure, while the lib binary on its own read 183 passed, 0 failed and 1 ignored in three further
runs that morning.
`cargo test` never builds the helper, and which exe `FB_ARCHIVE_HELPER` names is part of the
answer: the framework-dependent Release tree here was older than `helper\FbArchive\ParentWatch.cs`,
so `helper_lifetime` read the helper alive on a live pipe — the very shape the watch was written
against — until `dotnet build helper/FbArchive -c Release` (44 s) brought it current, and both of
its scenarios answer green since. The watch sits in `fb-archive.dll`, not in the 158 KiB apphost
beside it, so searching that exe for `ParentWatch` says nothing either way; the hand check against
a current build was that `fb-archive` ended 1 ms after the instance it was told to watch.
One ordering note for a full run: cargo stops at the first failing target, and
`tests\ui_selftest.rs` sorts before
`tests\win_startup.rs`, so a UI failure hides the targets behind it — pass
`--no-fail-fast`, or run what follows separately as those two scenarios were. The helper's own
suite (`dotnet test helper/FbArchive.Tests`) answers 517 passed, 0 failed, 1 skipped
(`Remove_IsCaseSensitive`) on this host, measured 2026-11-12 in 13 m 31 s — it is slow, and it
builds the same vendor trees cargo does, so it gets a run of its own rather than a concurrent one.
The launcher's own crate answers a further seven scenarios (`cargo test --release` in
`winpack\onefile`, which needs `FB_PAYLOAD` naming a staging zip because its `build.rs` insists on
one) — 7 passed, 0 failed measured 2026-11-12; it is a separate crate, so they are not part of the
238. What they cannot reach was measured on the desktop instead: that packaged launcher unpacked a
fresh folder in 6 s and refreshed a folder stamped with an older id in 10 s, in both cases with no
staging folder left and the unpacked program reading subsystem 2, and against a folder an instance
was running from it answered a window naming the folder and the system's reason within 3 s, the
folder's 2,882 files reading identical afterwards. Run against a build with the writability check
left out, the same start replaced every one of those files under the running instance, started a
second instance and left `.payload-staging` in the folder — the control that shows what the check
prevents.
These failures surfaced when the app's Windows build compiled again
after the `--lvm` helper stopped importing Linux; no Windows build had run these Linux-era
update and drives paths before.
