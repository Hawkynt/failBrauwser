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
sit next to the file itself. The copy is a plain `Copy-Item -Force` over the existing tree and
`install.ps1` stops nothing: an instance that is running holds its own image, so it has to be
closed before the install reaches that prefix — otherwise the one file the tree is named after is
exactly the one that cannot be written.

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
instance lives it stops 45 s after its last request. [TOOL-STARTS.md](TOOL-STARTS.md) audits every
other program the release starts — the version-control clients, `curl`, the update's swap script,
the elevation helper — and what bounds each of them on both systems.

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
| Volume name, format, device icon (README: *Drives*) | UDisks2 `Filesystem` and `Format` methods, `mkfs.<type>`, `e2label` and friends | `SetVolumeLabelW`, `format.com` run by an elevated copy of this program, the icon read back through `SHGetFileInfoW` |
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
- **Elevation on Windows is one consent prompt per act, never a service and never at startup.**
  Renaming the label of a volume that is not yours to write, and formatting, run this program's own
  file again with the verb `runas` (`ShellExecuteExW`) and the parameters `--rename-volume …` /
  `--format …`, each told by `--outcome <path>` which file to write its answer into; the window waits for
  that run and reads the file back. Cancelling the consent dialog is a no-op and leaves nothing running,
  and the run is started hidden, since it has nothing to show the user beyond the consent dialog itself. The Linux twin of the same two acts goes through the
  `pkexec` helper that Backup Mode already uses (`src\links\helper.rs`).
- **The arguments of that run travel quoted, and this machine's own parser answers them back unchanged.**
  `ShellExecuteExW` takes the whole tail of a command line as one string, so `src\win\elevate.rs` builds it
  by the rule `CommandLineToArgvW` reads. Measured 2026-11-18 in this checkout: ten arguments — a bare drive
  letter, a `/FS:` switch, a `/V:` label holding spaces, an empty argument, a temp path, a name ending in a
  backslash, one holding quotes, one holding a tab, one ending in a backslash before a quote, and a label of
  the format act itself — were round-tripped through that parser and came back exactly as they went in
  (`win::elevate::tests`). The consent prompt itself has been answered nowhere yet: the elevated half of both
  acts is built, and it stays recorded as not run until the authorized format step reaches it.
- **A refused label write comes back as a code, and only one code means the rights are missing.** Measured
  2026-11-18 on an un-elevated instance: `SetVolumeLabelW` for `Z:\` — a letter this machine names no drive
  for — answers a code that is neither 0 nor 5. `fsformat::act` therefore sends only win32 5
  (`ERROR_ACCESS_DENIED`) to the privileged run, which is what a stick never answers and an internal volume
  does, and tells every other code as the machine's own refusal of that name or that volume.
- **What the format window offers is what this build's `format.com` offers, and the parameter set is
  closed.** Measured 2026-11-18 from that tool's own usage text, which is where the README's option
  table gets its switches, its per-file-system value lists (the allocation-unit ladders, the
  `enable`/`disable` state words, the UDF revisions), its "only NTFS / only ReFS" markers and its
  cluster-count rules for FAT and FAT32. The two paths that can be asked **disagree**, and that is
  recorded rather than resolved: `format`'s text lists an 8 KiB allocation unit for NTFS while
  `Format-Volume -WhatIf` (which writes nothing) refused exactly that for the 64 GB removable stick as
  `Invalid Parameter`; `Format-Volume` refuses FAT for that stick as `Size Not Supported`, refuses
  ReFS for it as *the specified file system is not supported*, offers FAT32/exFAT/NTFS there, and
  names no UDF parameter at all. The window builds the command for `format`, because `format` is the
  program that runs, and whatever refuses quotes its own words. There is **no per-volume MFT
  reserve** on this Windows: `fsutil fsinfo ntfsinfo d:\` reports an MFT zone (224 MB on this machine)
  that `fsutil behavior set mftzone` sets machine-wide (`MftZone = 0` → 200 MB), and `format` has no
  per-volume switch for it, so the NTFS space knob the window offers is the log size (`/LogSize:`) and
  the zone is never written.
- **The list of file systems comes from the tool that will run, because no Windows query answers the
  question.** Measured 2026-11-18: `FSCTL_QUERY_FILE_SYSTEM_SUPPORT` sent to `\\.\C:` and `\\.\F:`
  answers `ERROR_INVALID_FUNCTION` (win32 1) with a rights-free handle and with `FILE_READ_ATTRIBUTES`
  alike, so it says nothing about what a volume could be formatted to — the offer is read from
  `format.com /?` once per run — asked by that file name, since a start without an extension gets `.exe`
  appended and asking it as `format` answers that it is not installed — kept, and narrowed per volume by
  the README's size and kind rules, and a
  word that module's table does not know is simply not offered. What the machine does answer about file
  systems is which drivers it carries, and that answer **disagrees** with the tool: the registry holds
  `fastfat`, `exfat`, `ntfs` and `refs` (Start 3, loaded on demand) but `udfs` with **Start 4 =
  disabled**, while the same tool names UDF in its list. Whether a UDF format on this machine is
  therefore refused is not tested here — the authorized format step reformats the stick to NTFS — so the
  disagreement is recorded and UDF stays offered because the tool offers it, with the OS answering in
  its own words if the driver refuses.
- **A volume's size is the file system's own total, read where the machine answers.** Measured
  2026-11-18 on an un-elevated instance: `IOCTL_DISK_GET_LENGTH_INFO` on the volume handle `\\.\A:` and
  on `\\.\F:` answers `ERROR_ACCESS_DENIED` (win32 5), and the same query on `\\.\PhysicalDrive1` answers
  the same, while `GetDiskFreeSpaceExW` answers a total for every lettered volume here (A:
  483 183 554 560, C: 482 226 466 816, D: 536 868 814 848, F: 64 143 622 144 and the two mapped shares
  15 698 757 677 056). So the drives page takes that total as the volume's size — which is what the
  format page narrows its file systems by — and the *device's* length, which the device icon prints, is
  read only where the machine allows it; where it is refused the shape's evidence line says which of the
  two sizes is shown rather than passing a volume's size off as a device's. That stick answers FAT32 with
  no volume label today, and that is the state the authorized format step reads back before it writes and
  restores afterwards.
- **The format gate names the volume Windows runs from from the machine's answer, not from the drives
  model's `system` flag.** The flag exists to hide the unmount button of an internal fixed volume, and
  measured here three lettered fixed volumes answer it while only one of them holds Windows (its own
  directory sits on `C:\Windows`), so refusing on the flag would refuse data volumes the README offers a
  format for. The gate asks where
  Windows' own directory sits and refuses that letter; a boot or recovery partition with no letter cannot
  be reached from the page at all, and where a lettered volume is held by the system it is `format` that
  answers, in its own words, which the result window quotes. The same reading names what would have to be
  refused beyond it — `Win32_OperatingSystem` answers `BootDevice=\Device\HarddiskVolume1` beside
  `SystemDrive=C:` on this machine, and that first volume carries no letter, so the page cannot reach it.
- **Not measured here yet:** the label ceilings the README names. FAT's 11 characters and NTFS' 32
  are what the drivers and the tools have always taken, exFAT's 15 and UDF's 32 are the file systems'
  own documented fields, and `Format-Volume -WhatIf` validates no label length at all — it took a
  33-character label for exFAT and FAT32 on the stick without a word — so the entry enforces the
  table's number and the authorized format step reads back what the tool actually wrote. Also not
  measured yet: which of the four device-icon hooks this Windows 11 build actually honours. The read-back
  of the drive's icon through `SHGetFileInfoW` and of the name shown under it is the measurement that
  settles it; until that line stands in this file the hook list is unverified,
  and the device-description section names of `autorun.inf` (the 2011-era schema) are the part most
  likely to answer nothing at all.
- **The Linux half of this feature has never been run here** — there is no Linux on this host.
  `e2label`, `exfatlabel`, `ntfslabel`, `mlabel`, `swaplabel` and `mkfs.<type>` behind the
  administrator helper, with the option table read from each tool's own usage text and the built table
  as the fallback: written to the same shape as the Windows half, recorded as unverified, and its
  starts named in `docs\TOOL-STARTS.md`.
- **A name longer than the file system's own label holds is planned, not dropped.** The short variant the
  README describes (forbidden characters dropped, FAT's label upper-cased, a cut inside a word pulled back
  to the word boundary) is what `fsformat::name_plan` computes, and both windows show it beside the entry
  you type into: the properties row says *the volume itself carries "SAMSUNG 64" — FAT holds 11 characters
  at most*, and the format window says the same in its own words. Those are the words a scenario reads
  (`tests\drive_format.fbt`), and the plan is pinned by `fsformat::tests` for the models this machine
  reports. **What writes the pinning files is not built yet**: nothing of this feature writes
  `autorun.inf` or `desktop.ini` today, so the long name survives as the name the windows call the volume
  and as `NamePlan::called`, and the writing of those two files — with `folder.ico` and `DevIcon.fil` — is
  the commit after this one. **Which of the two pins this Windows answers to is not known here**:
  `SHGetFileInfoW` and the name shown under the drive are read back at the authorized format step, and if
  neither pin shows the long name that is what gets written down, not a claim that it works. The short
  variant is deterministic rather than the volume's serial-numbered `8.3` form, so the same long name
  always yields the same label.
- **The device-icon half of the README's promise stands unbuilt.** *Create device icon* and *Remove device
  icon* are on no drive menu and no properties window, the format window has no *Write a device icon* row
  and no *Device shape* dropdown, and no glyph of the README's nine shapes is in the tree. The words the
  format window does propose as the volume name come from `fsformat::proposed_name`, which is built and
  tested on its own; the picture, the twelve images of `DevIcon.fil` and the four files that carry them are
  the next commit, and nothing about the shell honouring them has been measured.
- **No automated run writes a volume or elevates.** The format scenario drives a stand-in volume that names
  no drive letter and holds no mount point, so the act answers before a start of any program exists — on
  Windows *This volume has no drive letter to format.*, and over there the switches of the table belonging
  to this system's tool — and the scenario reads that answer out of the window it is told in. No consent
  dialog appears and no format tool is reached, so that scenario needs no stand-in act program; the
  `FAILBRAUWSER_ELEVATE` seam (`src\fsformat\act.rs`, shaped as `src\links\helper.rs` already uses) stays
  pinned by its own lib test, and a run that records the argv an act program was handed comes with the
  icon step. The single real format in this project's verification is the one authorized step on the 64 GB
  removable stick, recorded where it was measured.

## Test suite state on Windows

`cargo test` runs everything; the `.fbt` UI suite runs through `tests/ui/harness.ps1`.
Scenarios that need Linux-only services skip with a printed reason:
`portal.fbt`, `portal_frontend.fbt` (XDG portal), `mount_image.fbt` (UDisks loop),
`desktop_links.fbt` (roaming-profile Desktop, see above), `summary.fbt` (symlink fixture),
and the portal/default-app parts elsewhere. Timing budgets for the big-listing paints are
widened for win32 GTK plus Defender (`big_listings.fbt`) — measured here, not guessed.
That made 45 scripts across 46 scenario tests — the tree carries 61 scripts and 52
scenario tests in `tests\ui_selftest.rs` today, the numbers further down being today's; run serially (`RUST_TEST_THREADS=1
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

The line came back on 2026-11-17 as a fresh clone of the online repository, and it did not build here.
`cargo check --release --all-targets` answered E0308 at `src\links\backup.rs:279`, where windows 0.62 wants
`Option<PSID>` for the owner and group of `SetNamedSecurityInfoW` — 0.62.0 and 0.62.2 declare it the same way, so
that Windows-only path had never compiled anywhere — while `tests\install_script.rs` (`std::os::unix` ungated) and
`tests\ui_selftest.rs` (the `#[cfg(unix)]` `have()` called from ungated code) did not compile on Windows at all, and
`src\links\siblings.rs` imported an `Ordering` only its not(windows) arm uses. A test target that fails to compile
ends `cargo test` before a single scenario runs, so none of the numbers below were reachable until those were
repaired (`1b6b141`, `71196d9`, `1845eb7`, `4da114a`) and the sysroot pin followed the mirror (`8e20fe9`: its
database no longer carries pkgconf 1~3.0.10-1, and only the two roots are pinned — of the 63 files
`bootstrap-lock.txt` recorded on 2026-11-10, eight have moved to a newer version in the meantime (openssl 3.6.5 →
4.1.0, gcc runtime 16.2.0-6 → -7) and one, libb2, is not pulled any more, so that sysroot is no longer
reproducible from the tree). The vendored registry generator went back to the Roslyn this SDK loads, CS9057
otherwise (`e491362`), and `src\win\storage.rs:207` stopped denying clippy's `absurd_extreme_comparisons`
(`bb3848a`: `header.Size` is a u32, so `<= 0` never guarded and no other lint of the crate was reported at all).

Measured on that base, `cargo test --release --no-fail-fast` read 446 passed, 19 failed and 2 ignored with the tree
as it arrived, and 455, 14 and 2 after the Blame work below — the same scenarios plus the four new ones, none lost.
Three of the 19 are lib scenarios asserting Linux facts to a Windows host:
`actions::tests::terminals_debian_and_others` wants `Some("x-terminal-emulator")`,
`lvm::tests::admin_tools_are_found_in_sbin` splits a Windows `PATH` into its sbin list, and
`links::tests::the_folder_s_filesystem_is_read_by_statfs` expects an unreadable `FsCaps` where NTFS answers every
capability. Run serially the UI target reads 49 of 52 green in 286.87 s: `custom_actions` and
`encrypted_drive_with_lvm_inside`, named in this section for as long as it has existed, plus `archives_as_folders`
of the clipboard group; in the parallel shape the same target read 36 of 52 before and 41 after. That group moves
with the machine rather than with the tree: `undo_operations` failed four runs in a row run by itself —
`wait-exists …\dest\a.txt` with `jobs busy false`, the paste never starting — while answering green in every suite
run and in the serial one.

Re-measured serially on 2026-11-17, with the repairs further down this section in place: **48 of 52 green in
315.04 s**. Four fail: `custom_actions` and `encrypted_drive_with_lvm_inside`, which this section has named for as
long as it has existed; `big_listings_do_not_freeze_the_window` at 14 499 ms against its 12 000 ms win budget, inside
the spread this section already records for that scenario; and `queued_paused_and_verified_copies` of the clipboard
group. That group moved exactly as recorded: nine failures of one serial run re-ran together on the same day and
answered 5 green in 177.69 s — `file_ops`, `undo`, `queue`, `share_refresh` and `checksums` back — while the three
kinds above stayed. One more left that list for a reason of its own, below.

A step that reads *inside* an archive asks this program's C# helper, and `archive::locate_helper`
(`src\archive\client.rs:304`) answers the bare name `fb-archive` when it finds no file of its own — the start then
fails, the step finds nothing, and nothing is said about it. `search.fbt` met exactly that here on 2026-11-17: three
runs failed its `wait-row inside-report.txt` with `0 rows` (once alone, in 26.82 s), and the same scenario answered
green as soon as a built helper was named — 38.37 s beside `archives_as_folders`, which answered green in both shapes,
with no helper reachable and with one, so the scenarios that open an archive as a folder do not hang on it while the
archive leg of a search does. The target names
that helper itself now (`archive_helper` in `tests\ui_selftest.rs`: `FB_ARCHIVE_HELPER` if it points at a file, else
this checkout's own `helper\FbArchive\bin\…\fb-archive.exe` or `target\helper\`), and prints that there is none to
name rather than letting the step go quiet. The other half of that finding is still open: with *search inside
archives* ticked and no helper reachable, the window shows an empty result list and no word about why.

Nine scripts run through `tests\ui_git.rs` and `tests\ui_vcs.rs`, and since `da74ef6` both targets run on Windows
too: their guard used to ask for `sway` or `tilewin` *and* `grim` plus `dbus-run-session` and to call
`tests/ui/harness.sh` wherever it stood, while `tests\ui_selftest.rs` had the `#[cfg(windows)]` branch reaching
`tests/ui/harness.ps1`. `have()` answers for the platform now, the fixtures build their working copies from
arguments rather than shell text, and each platform calls its own harness. Measured here on 2026-11-17: the six Git
scenarios green — `git_branches`, `git_commit`, `git_log`, `git_overlays`, `git_remote` and `git_tools`, the target
in 141.09 s — and `tests\vcs_backends.rs` 16 of 16 in 18.14 s, its Subversion legs running here for the first time
with TortoiseSVN's `svn` and `svnadmin` on `PATH`; `vcs_svn.fbt` green through `ui_vcs`, with `vcs_hg` and `vcs_cvs`
skipped and the missing client named. Two of those scripts could never have passed as written: the target asked for
`vcs_overlays.fbt`, a file this tree has never held (the scenario is `git_overlays.fbt`), and `git_remote.fbt`
compared the harness's own path spelling with what git shows for a remote URL, so a path expectation now reads the
same with either separator (`b0deba8`, `da74ef6`). Two fixture shapes needed the same kind of repair: a Subversion
`file://` URL of a Windows folder takes forward slashes *and* the third slash, or svn answers E170000 "Illegal
repository URL" (`e84f4aa`), and git refuses the null device as `GIT_CONFIG_GLOBAL` — `unable to access 'NUL':
Invalid argument`, measured for the bare name and for a rooted one alike — so a scenario keeps git away from the
user's configuration with an empty file of its own. What the harness answers reaches the test through a file under
the fixture rather than through a pipe, because a tool still running when the run ends holds that pipe open and the
test then waits for an end that never comes (`da74ef6`, `f14bffb`), and the whole-script budget is 180 s, a fourth
argument of the harness (`aba4d13`). The Blame defect survived all of that because it survived review: `src\ui\vcs\blame.rs` built its commit column as
`l.hash[..8]`, so a Subversion `3`, a CVS `1.2` or svn's empty uncommitted hash read "end byte index 8 is out of
bounds for string of length 1/0/3" and the window never filled, while git's forty characters hid it. The rule lives
in `BlameLine::short` now, with four lib scenarios and the mutation checks that kill it — index back and three of
the four panic, cut at seven and the long one fails (`21fdba7` for the README sentence, `4cc5cf2` for the code). A
Mercurial and a CVS client are still absent here, so those two tools keep their readings pinned by fixtures;
Subversion is measured now, in `tests\vcs_backends.rs` and through its own window scenario.

The helper's own suite answers 506 passed, 0 failed in 3 m 39 s on 2026-11-17, `Remove_IsCaseSensitive` printed as
skipped while that run's summary counts none; eleven scenarios fewer than the 517 recorded on 2026-11-12, that
being what the online tree carries. Neither dead-code warning of that base stands any more (`224816b`):
`user_media_base` (`src\lvm.rs`) reaches the LVM mount paths it was written for, and the passphrase question of
*Open as* asks through `kind_needs_passphrase` (`src\ui\extensions.rs:310`), which is where it is decided whether to
ask at all. Measured on 2026-11-17: the release builds of the `vcs_backends`, `ui_git`, `ui_vcs` and `ui_selftest`
targets carried no `warning:` line at all — which is what `winpack\build.ps1` needs, since it treats anything cargo
writes to stderr as fatal.

Measured on 2026-11-18 for the drives windows' two acts (README: *Renaming a volume*, *Formatting a
volume*). The new scenario `tests\drive_format.fbt`, driven as `renaming_and_formatting_a_volume` (the 53rd
test of the UI target), answers green and touches no volume of this machine: the stand-in it drives names no
drive letter and holds no mount point, so the act answers before any program is started, no consent dialog
appears and no format tool is reached. Driven serially with the four scenarios that share that page —
`drives_page`, `unmounted_drives_in_the_side_panel`, `phones_with_the_drives`, `disk_image_as_drive` — the
target reads **5 of 6 green in 37.52 s**. The one standing is `encrypted_drive_with_lvm_inside`, named in
this section for as long as it has existed, today stopping at its line 12
(`wait-side-drive Linux : Encrypted — click to unlock`, the entry that should come back after the
scenario's own *Unmount* step). That this is not the work above was measured rather than assumed: the same
script, the same space-free work folder and the same sysroot answered that same line 12 for a binary built
out of `git archive` of the commit this work starts on, exported into a scratch folder — the same failure in
the same place with no line of this work in it. The module suites behind those windows read **67 passed, 0
failed** across `fsformat::`, `win::elevate::`, `win::storage::` and `drives::`, and `source_rules`,
`win_startup` and `helper_lifetime` read 2, 3 and 2 green. `cargo clippy --release --lib` names 51 warnings
in the crate and none in `src\fsformat\mod.rs`, `src\ui\format.rs`, `src\ui\drives.rs` or
`src\ui\selftest.rs` — which is what a re-check with those four files touched answers, and the release build
of the same tree carried no `warning:` line either.
