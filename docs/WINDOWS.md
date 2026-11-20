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
exactly the one that cannot be written. That script runs in a terminal and leaves the closing to
the person running it; the launcher started from the desktop asks the running copies first, and the
in-app update asks them too (README: *Updating with more than one copy open*).

That unpack is all-or-nothing. The payload is staged in a folder inside the install folder and
its files are moved into place only once every entry has been read, inflated and CRC-verified,
so a payload that cannot be applied — a file that stands where one belongs, an entry that fails
its check — leaves the installed tree exactly as it was instead of replacing part of it, and the
staging folder is taken away again; `.payload-version` is written only after a complete unpack.
A launcher that cannot install says so where the person who started it will see it: a message
box naming the install folder and the reason when it was started with no console, and that same
text in the terminal when a terminal started it; either way it exits 1 rather than pretending to
have started. One boundary stays: a refresh needs every instance running from that folder closed
first, and the launcher asks for that close the way an update does — bounded, an ask and never a
kill, its refusal naming the process that still holds a file. Renaming a running program file aside
is possible — that is how *Help → Check for Updates*
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

### Building on Linux, testing under Wine

The same MSVC target builds on Linux, and its programs and test binaries run under Wine:

```sh
winpack/cross/setup.sh                                  # once, and after bootstrap-lock.txt changes
. winpack/cross/env.sh
cargo build --target x86_64-pc-windows-msvc             # target/x86_64-pc-windows-msvc/debug
winpack/cross/build-helper.sh                           # target/helper/fb-archive.exe
cargo test --target x86_64-pc-windows-msvc --lib -- --skip ops::delete::tests::trashing_a_local_file_lands_in_the_recycle_bin
```

`setup.sh` fills `FB_WINCROSS` (default `~/.local/share/fb-wincross`, which must hold no space):
the Windows standard library through `rustup`, Microsoft's CRT and Windows SDK through `xwin` (which
accepts their license for you), the files `winpack/bootstrap-lock.txt` pins, taken from
`repo.msys2.org` itself, their import libraries (`winpack/cross/genimports.sh`, the twin of
`genimports.ps1`, with `llvm-dlltool`), a Wine prefix with a loader cache written for that sysroot,
and MinGit's `git.exe`. `winpack/sysroot` becomes a link to that sysroot. `env.sh` sets cargo to link
with `lld-link`, compile C with `clang-cl`, embed the icon with `llvm-rc` (`FB_RC`) and run the test
binaries with `wine`. `build-helper.sh` publishes the archive helper for `win-x64` with NativeAOT, whose
cross-system refusal `DisableUnsupportedError` lifts: `ilc` emits the Windows objects and `lld-link` links them
against the same CRT and SDK (through `LIB`), without `/SOURCELINK` and debug symbols, which it cannot take.

What Wine cannot answer fails there and nowhere else: the recycle bin (`E_NOTIMPL`; the test named
above never returns), WPD devices, WNet shares, `format.com`, `Z:` (Wine's drive for `/`), the hidden
and system attributes on a Linux folder, a rename that changes only case, PowerShell (the update's
swap script), `tests/git_repo.rs`, whose `sh` from Git for Windows starts no program under Wine, and `tar`
(one fixture of `tests/archive_helper.rs`). The archive helper's ciphers are its own — Wine's `bcrypt`
refuses ECB, which the encrypted volumes step through — so those volumes open under Wine as on Windows.
The three Linux-only tests of the lib fail there as they do on Windows.

## Updating

*Help → Check for Updates* on Windows installs the newest release's ready-built
`failbrauwser-<version>-x86_64-windows.zip` (the same name formula `package.ps1` produces).
Every running copy is asked to close and to write down its windows and tabs first (README:
*Updating with more than one copy open*), and the files are replaced only once nothing in the
install folder is open: the fresh `bin`/`lib`/`share` contents are copied in, the program file
that was there is deleted, and the new program is started with the remembered windows handed to
it. Renaming the image aside — Windows allows renaming a running image, not overwriting it —
is kept only as what it is, a way past a copy that would not close, and the waiting before the
swap is bounded: a copy still holding the folder stops the update by name instead of being
killed. The swap script is detached on Windows as `setsid` detaches it on Linux, because the
program that started it is one of the files being replaced. The *build from source* channel is
Linux-only there (it runs `install.sh`); a copy installed machine-wide shows the manual route
instead (`winpack\install.ps1`).

## What is where

| Area | Linux | Windows |
|---|---|---|
| System calls under copy/delete/trash | `osprim` errno predicates | same file, Win32 codes through `raw_os_error` |
| Drives, mount state | UDisks2 / GVfs | `win::storage` (drive types, geometry, SSD/HDD), WNet for shares |
| Volume name, format, device icon (README: *Drives*) | UDisks2 `Filesystem` and `Format` methods, `mkfs.<type>`, `e2label` and friends | `SetVolumeLabelW`, `format.com` run by an elevated copy of this program, the icon read back through `SHGetFileInfoW`, and those four files read back by this program itself for its own drive rows, within the bounds *Device icons* names |
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
  Renaming the label of a volume that is not yours to write, formatting one, and writing or removing the
  four files of a device icon where the volume refuses the write, all run this program's own file again
  with the verb `runas` (`ShellExecuteExW`) and the parameters `--rename-volume …` / `--format …` /
  `--device-icon …` / `--remove-device-icon …`, each told by `--outcome <path>` which file to write its
  answer into; the window waits for that run and reads the file back. A format whose volume name holds a
  space names that name to the privileged run as `--volume-name <name>` instead of as the tool's `/V:`
  switch, which this build's tool refuses (measured, README), and the run writes it once the format has
  worked — one prompt either way, and the format command itself keeps to switches. A rename that has to pin a
  name longer than the file system holds carries what the privileged run needs for that pin beside the label —
  the folder to write into, the name as it was typed, the file system's word, the model and the size the
  description words are drawn from — so the run that takes the label writes the two text files too and naming a
  volume never answers a consent dialog twice (README: *Renaming a volume*). The two device-icon verbs ask only
  once the write itself answers that the rights are what is missing, so a volume this user may write to is
  settled with no prompt at all. Cancelling the consent dialog is a no-op and leaves nothing running, and
  the run is started hidden, since it has nothing to show beyond the consent dialog itself. The Linux twin
  of the same acts goes through the `pkexec` helper that Backup Mode already uses (`src\links\helper.rs`).
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
  program that runs, and whatever refuses quotes its own words. Measured 2026-11-19, that program's own
  parameter check settles the disagreement for the window: asked on a letter that holds no volume, `/A:`
  answers with the drive lookup for 512, 1024, 2048, 4096 and 8192 of FAT, FAT32, exFAT and NTFS alike, and
  answers `Unzulässiger Parameter` for 12 288, 16 384 and every larger value **before** any drive is opened —
  the same ceiling for every file system it names, which leaves ReFS with 4096 alone. The window's ladders
  follow that acceptance rather than the usage text, so no choice in the dropdown can only end in a refusal;
  reaching 16 KiB and above stays with the machine's own disk tools, which this feature never calls. There is **no per-volume MFT
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
  483 183 554 560, C: 482 226 466 816, D: 536 868 814 848, F: 64 157 253 632 and the two mapped shares
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
  table's number and the authorized format step reads back what the tool actually wrote. **One of the
  device-icon hooks is measured: the folder's display name.** A folder carrying the files this feature writes
  — `desktop.ini` in UTF-16 with its mark, the folder itself marked system — is called by `SHGetFileInfoW`
  under the pinned name and not under its own, and after the removal the same reader calls it by its own name
  again with the two attributes back where they were (2026-11-18, `fsformat::icon` driven against a scratch
  folder outside the repository, deleted again). The attribute and the encodings were asked about on their
  own, on folders carrying a hand-written `desktop.ini` in each spelling: the pin is ignored the moment the
  system attribute is taken off the folder, and a `desktop.ini` written as UTF-8 shows the name a single-byte
  reader makes of it (`223.246` for `ßö` against `195.376.195.182`), while UTF-16 with its mark and the
  machine's own codepage both show the name whole. A folder carrying the section of ours and no pinned name
  shows its own name, so that section disturbs nothing of the shell's own keys. Still unmeasured is the rest
  of the hook list — `autorun.inf`'s `icon=` and `label=`, and the `DevIcon.fil` device-image hook, which
  belong to a drive rather than to a folder and are read back at the authorized format step — and the
  device-description section names of `autorun.inf` (the 2011-era schema) stay the part most likely to answer
  nothing at all.
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
  reports. `fsformat::icon` writes both pinning files now — `desktop.ini` in UTF-16 with its byte-order mark,
  `autorun.inf` in plain ASCII, both carrying the section of ours that *Remove device icon* removes by — and
  the folder display name they produce is measured above. **Which pin a mounted volume answers to is
  still not known here**: `SHGetFileInfoW` and the name shown under the drive are read back at the
  authorized format step, and if
  neither pin shows the long name that is what gets written down, not a claim that it works. The short
  variant is deterministic rather than the volume's serial-numbered `8.3` form, so the same long name
  always yields the same label.
- **The picture, the icon file it goes into, and the four files of one device icon are built, and this
  Windows reads them.** `src\fsformat\art.rs` composites one of the nine tracked glyphs —
  `assets\device-glyphs\`, drawn by the `make-glyphs.ps1` that sits beside them, none of another program's
  artwork — with the words the machine reported about the device, and `src\fsformat\ico.rs` writes the
  twelve images the README names: 16, 32, 48 and 256 pixels at 24-bit colour with an 8-bit alpha, at an
  8-bit palette and at a 4-bit palette, each with its one-bit mask. Measured 2026-11-18 with `ExtractIconExW`,
  which is the shell's own reader of an icon file, and with GDI+ as a second opinion: a file this writer
  produces answers `extract-count=1` — one icon group — and extracts a 32×32 large and a 16×16 small icon,
  and GDI+ opens it at 32×32; the tracked `data\icons\failbrauwser.ico` answers the same way, and so does a
  one-icon file GDI+ wrote itself, so the reader is not merely agreeing with this writer.
  Two findings came out of that probe and are pinned by tests rather than remembered.
  **An entry names its image's length before its place.** With the two words exchanged the pictures were
  the same, the directory read back plausibly to a reader that mirrored the writer, and every product file
  still answered `extract-count=0` while GDI+ refused to open it; in the documented order the same pictures
  extract. A test that reads the file with the writer's own misreading proves nothing, so
  `fsformat::ico::tests::an_entry_names_the_length_of_its_image_before_its_place` reads the two words from
  their documented offsets and asks the image's own header whether it is that many bytes long.
  **A four-bit row of a side that is no even number has one nibble over**, and that nibble names the
  transparent slot: filling it from the picture instead ran past the picture's own pixels and panicked on
  the last row of a 17-pixel picture, which is what
  `a_picture_of_an_odd_side_still_writes_every_row_of_its_four_bit_image` was written to fail on first.
  A temporary bisect — not a kept test — settled three further spellings: a paletted entry whose header
  leaves `biClrUsed` at zero counts nothing at all for 8-bit and is read for 4-bit, and a 32-bit image with
  its mask named in the header's height extracts whether the directory names its planes and bits or leaves
  them zero, so this writer names the palette count and carries the mask for every image.
  `src\fsformat\icon.rs` writes the four files of one device icon — `DevIcon.fil` and `folder.ico` with the
  bytes that container produced, `autorun.inf` in plain ASCII and `desktop.ini` in UTF-16 with its mark —
  marks them hidden (the two text files system as well), marks the root system-and-hidden only where the
  system attribute was not already there, and says in the marker which of the two it did; a create that
  cannot finish its four files takes back the ones it wrote. Its removal reads the two text files and deletes
  only a file carrying the section of ours and the icon names that file points at, so a foreign
  `autorun.inf` keeps every byte it had and an icon file no marked file names stays where it is; a name
  walking out of the root is not obeyed. The display name that pair produces, and the removal taking it back,
  are the folder measurement above. That module answers what those files **say** as well as what they cost:
  `icon::shown(root)` reads the two text files, honours only one carrying the section of ours, and answers the
  long name pinned there and the picture file that marked file names — the same answer the drives page row, the
  side panel's drive row and the properties window name and draw the volume by, and the reason a foreign
  `autorun.inf` lends this program neither a name nor a picture. The pinned name wins ahead of the volume's own
  label, ahead of the mount point's name and ahead of the image file a mounted image is called by, because it is
  the volume's own statement about itself. The picture's bytes reach the row through the ICO loader this payload
  ships (`Pixbuf::from_read`), and a file that will not decode leaves the themed drive icon in place. What that
  reading is given is bounded by this module and not by the file: `icon::marked_text` reads one of the two text
  names no further than 64 KiB and answers nothing past it — the listing and `remove`/`name_pins` alike, so a
  planted giant is neither read nor called one of ours — `pin_worth_shown` refuses a pinned name that is empty,
  longer than 255 characters, or carrying a control character or a directional override, and
  `picture_can_be_shown`/`picture_bytes` ask `ico::pictures_are_ones_a_file_can_hold` whether an icon file's own
  directory and image headers agree and stay inside the file and name no image past 256 pixels, within an 8 MiB
  bound, reading headers only and reading the bytes through the very handle the answer came from. A named picture
  failing that answer is skipped for the next one the marked file names, and none leaves the themed icon; each
  answer falls back on its own. The scenario `device_shown.fbt`, run as
  `a_device_shows_what_its_own_files_name`, pins that reading over a stand-in stick whose folder holds the four
  files this module wrote, and over a second one whose two picture files hold nonsense: it is still named by its
  pinned name and draws no picture, and *Remove device icon* takes its names away the same way. Those two verbs are reached now: *Create device icon* and *Remove device
  icon* stand on a drive's row menu and on its properties window — shut, with the act's own reason on their
  tooltip, wherever a volume names no folder or is read-only — and the format window carries the *Write a
  device icon* box with the *Device shape* dropdown beside the words this machine reported. The scenario
  `drive_format.fbt` drives that create and that removal against a stand-in image mounted into a folder of
  the run's own, and reads back the four files, the answer naming them, and the removal's words. Still
  unmeasured is what those files do on a **mounted volume of this machine**, where `autorun.inf`'s `icon=`
  and `label=` and the `DevIcon.fil` hook belong: that stays for the authorized format step.
- **No automated run writes a volume or elevates.** The format scenario drives a stand-in volume that names
  no drive letter and holds no mount point, so the act answers before a start of any program exists — on
  Windows *This volume has no drive letter to format.*, and over there the switches of the table belonging
  to this system's tool — and the scenario reads that answer out of the window it is told in. No consent
  dialog appears and no format tool is reached, so that scenario needs no stand-in act program; the
  `FAILBRAUWSER_ELEVATE` seam (`src\fsformat\act.rs`, shaped as `src\links\helper.rs` already uses) stays
  pinned by its own lib test. The device-icon half of that same scenario does write, and writes four real
  files — into a folder of the run's own, which a stand-in image of that run is mounted into. That folder
  needs no rights beyond the ones this program already has, so the write reaches no elevation, no consent
  dialog and no act program either, and nothing of this machine's volumes is touched. The single real format
  in this project's verification is the one authorized step on the 64 GB removable stick, recorded where it
  was measured.

## Test suite state on Windows

`cargo test` runs everything; the `.fbt` UI suite runs through `tests/ui/harness.ps1`.
Scenarios that need Linux-only services skip with a printed reason:
`portal.fbt`, `portal_frontend.fbt` (XDG portal), `mount_image.fbt` (UDisks loop),
`desktop_links.fbt` (roaming-profile Desktop, see above), `summary.fbt` (symlink fixture),
and the portal/default-app parts elsewhere. Timing budgets for the big-listing paints are
widened for win32 GTK plus Defender (`big_listings.fbt`) — measured here, not guessed.
That made 45 scripts across 46 scenario tests — the tree carries 62 scripts and 54
scenario tests in `tests\ui_selftest.rs` today (one of each being `drive_format.fbt` and its test, added by
this feature), the numbers further down being today's; run serially (`RUST_TEST_THREADS=1
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
folder's 2,882 files reading identical afterwards. That measured answer is the one from before the
ask: it refused without asking first, and the refusal named the system's reason rather than the
process holding the file. Run against a build with the writability check
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
volume*). The new scenario `tests\drive_format.fbt`, driven as `renaming_and_formatting_a_volume`, answers
green and touches no volume of this machine: the stand-in it drives names no
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
in the crate and none of them in `src\fsformat\` — which is what a re-check with that module touched answers;
the files under `src\ui\` are no part of that target, so they are measured with the binary below. The release
build of the same tree carried no `warning:` line either.

Measured again on 2026-11-18 for the device-icon verbs and the format page's box (README: *Device icons*).
The scenario reads green with its create and removal legs, and driven serially with the three drive scenarios
that share those windows — `drives_page`, `drives_in_the_side_panel` and `unmounted_drives_in_the_side_panel`
— the target reads **4 of 4 green in 18.79 s**. The whole lib of the same tree reads **392 passed, 3 failed,
1 ignored**, and those three are the Linux-only assertions named further down this section —
`terminals_debian_and_others`, `the_folder_s_filesystem_is_read_by_statfs` and
`admin_tools_are_found_in_sbin` — which is their state here before this feature and after it; the three guards
read 3, 2 and 2. `cargo clippy --release --bins --lib` names
**168** warnings across crate and binary, of which `src\fsformat\` and `src\ui\format.rs` carry none, and
`src\ui\selftest.rs` and `src\ui\drives.rs` carry four and seven — every one on a line this feature never
touched, blamed to commits of 2026-09-29 through 2026-11-14; the two findings that did stand on lines of this
feature's own code (a collapsible `if` in `src\ui\format.rs`, three needless borrows in
`src\ui\selftest.rs`) were cleared rather than left, and the older ones were left where they are.

Measured on 2026-11-19 by the **authorized format of the 64 GB stick** (README: *Formatting a volume*,
*Device icons*), which is the step the plan named and this section had been waiting for. The acts were run
through the feature's own code — a scratch driver outside the repository, deleted afterwards, calling
`fsformat::act::format` and routing the privileged half through `act::is_act` exactly as `src\main.rs` does —
against the real volume Windows names `F:` and enumerates as bus `usb`, model `Samsung Flash Drive`. Three
consent dialogs were answered, one per privileged run; nothing ran as a service and no instance was left.

- A format to **NTFS with a 2048-byte allocation unit**, quick, and the name `Samsung 64 GB`: the tool's own
  words came back, then *the volume's name is now Samsung 64 GB.* Read back: `FileSystem NTFS`,
  `fsutil fsinfo ntfsInfo` answering **2048 bytes per cluster** (so the unit the table settled reached the
  machine and the machine wrote it), `FileSystemLabel Samsung 64 GB`, capacity 64 160 399 360. The name holds
  a space, so it travelled as `--volume-name` and the format command held switches alone — which is the route
  the measured `/V:` refusal forced.
- A format to **exFAT at 8192 bytes with the name `FBTEST`**, which the tool's own switch can carry: read back
  `Label FBTEST`, `BlockSize 8192`, capacity 64 127 893 504. So both name routes are proven on a real volume,
  the tool writing one and this feature writing the other.
- The **four files of a device icon** on that fresh volume: `autorun.inf` 223 B and `desktop.ini` 298 B both
  `Hidden, System, Archive`, and `DevIcon.fil` with `folder.ico` at 408 966 B each `Hidden, Archive`. The
  container answers **twelve images — 256, 48, 32 and 16 px in 32-, 8- and 4-bit** — exactly the rule the
  README states. `autorun.inf` names `ModelName=USB stick 64 GB` and `Manufacturer=Samsung`, drawn from what
  the machine says, and `desktop.ini` carries `LocalizedResourceName` and `IconResource`.
- What **this Windows honours of those four hooks is nothing for the drive itself**: `SHGetFileInfoW` answers
  the generic drive icon for `F:\` (index 217 in the shell's image list, while the volume's own `folder.ico`
  holds index 218 in the same list), `ExtractIconExW(F:\)` answers **0** icons, and My Computer names the item
  `Samsung 64 GB (F:)`, which is the volume's own label rather than either ini file's word. The root already
  stood `Hidden, System` before anything was written, which is why the marker in both files reads
  `RootMarked=0`. That the icon and the localized name are read when the volume is browsed as a folder was
  **not** observed here — no Explorer window was screenshotted — so the honest statement is: the files are
  written as the rule names them, and on this build the drive's picture and name come from the label and from
  the shell's own icon.
- The **label ceilings** of the same volume, asked with the call this feature uses: exFAT takes eleven
  characters and refuses a twelfth (win32 154), NTFS takes 32 and refuses a 33rd.
- The restore: **131 072-byte clusters are not reachable through `format`** (its parameter check refuses
  them, README), so the stick went back through this machine's own `Format-Volume`, which reproduced the
  state it was in before this step exactly — label empty, exFAT, `BlockSize 131072`, `Capacity
  64 157 253 632`, `FreeSpace 64 156 598 272`, its root holding only `System Volume Information`. The four
  icon files were wiped by these formats, and the baseline held none.
- One fact of the scratch setup is worth keeping, because it cost a step: the privileged start receives
  **the shell's environment, not the parent's `PATH`**, so the scratch driver needed the GTK DLLs beside its
  own file before the elevated half could load. That is a fact about a scratch tree next to a sysroot; the
  installed image always sits next to its DLLs, which is why the product never meets it.

The same day's gate, and the install this feature had to reach. `cargo test --release --lib` answers **396
passed, 3 failed, 1 ignored** — the three failures are the Linux-only assertions named above and were the
same three before this feature began. The guard targets, run on their own because `--lib` failing stops cargo
before the targets behind it: `git_repo` 7, `helper_lifetime` 2, `source_rules` 3 and `win_startup` 2, all
green — the counts first written there in the order the targets were named belonged to other targets, cargo's
output having been read without naming which binary answered what, and the same four read green again in the
run named below, where each count is named with its target.
The four drives scenarios serial — `drives_page`, `drives_in_the_side_panel`, `unmounted_drives_in_the_side_panel`,
`renaming_and_formatting_a_volume` — **4 passed in 72.87 s**. `cargo clippy --release --lib --bins` names
**170** warnings; the two `constant is never used` warnings this feature's own module had left are gone, and
`src\fsformat` now answers none at all: the eleven standing in `src\ui\drives.rs` and `src\ui\selftest.rs` are
blamed by `git blame` to commits of 2026-09-29 through 2026-11-14, none of them to this feature.

`winpack\package.ps1 -SkipBuild` and `winpack\install.ps1` put that build where the user actually starts the
program: the per-user prefix, which the Start Menu entry names. Read back after the install — the stamp
`1.0.0 20261119T083900` beside the same id in `payload-id.txt`, `bin\failbrauwser.exe` 15 069 184 bytes, 47 DLLs
beside it, a loader cache of 105 lines rewritten for that folder, and the shortcut's target and icon both naming
that image. The proof it carries the feature is the scenario of the feature driven against **that installed
image** rather than against a build folder: `tests\ui\drive_format.fbt` through `tests\ui\harness.ps1` answers
**SELFTEST OK** — the typed Label row and its Rename button, the cut-at-a-word name and the refused one, the
format window's file-system dropdown and per-system option rows, the typed-letter gate, the device-icon box,
the act's own answer for a stand-in naming no drive, and the two device-icon verbs writing the four files into
a mounted image and taking them back out. The first such run failed at its 97th line — `select backup.ntfs:
selected 0 of 1` — and the reason was mine, not the image's: `harness.ps1` builds no fixture, and the cargo
wrapper creates `$T/inside` and `$T/backup.ntfs` before the scenario starts; with the fixture in place the same
script answered green, and the run's own *Remove device icon* step left the mounted folder holding nothing of
ours; the process census after that run named no `failbrauwser`, no `fb-archive` and no `format.com` left
behind.

The install was put again after the marker was fixed, so the image the user starts is the image every gate above
ran on: the stamp `1.0.0 20261119T104314` beside the same id in `payload-id.txt`, `bin\failbrauwser.exe` 15 047 680
bytes with **the same SHA-256 as `target\release\failbrauwser.exe` of this tree** — the byte-identical claim rather
than a rebuilt-and-hoped-for-one — `bin\fb-archive.exe` likewise identical to the built helper, 47 DLLs beside them,
the loader cache of 105 lines naming that installed folder and no path of the build machine, and the shortcut's
target and icon both naming that image. No instance was running when the folder was written, and none was left
after it.

Measured on that same volume on 2026-11-19 for **the pin of a name the label cannot hold** (README: *Renaming a
volume*), after the user had formatted the stick again. The rename went through the feature's own act — the same
scratch driver outside the repository, calling `fsformat::act::rename` against the real `F:` — and the volume was
put back by a second rename, so no format was needed for this step.

- A rename to the typed `Samsung 64 GB`: the act answered *the volume's name is now Samsung 64. The longer name it
  is called stands pinned in autorun.inf and desktop.ini.*, `Get-Volume` read the label back as `Samsung 64`, and
  the root held exactly two entries more than before — `autorun.inf` 180 bytes and `desktop.ini` 206 bytes, both
  `Hidden, System`. No `folder.ico` and no `DevIcon.fil` were written, and neither file names an image: the
  `icon=` line, the `IconResource=` key and the `Image=` key of the marker appear in no file this act wrote,
  exactly as the rule states. `autorun.inf` carries `label=Samsung 64` beside `ModelName=Samsung 64 GB` and
  `Manufacturer=Samsung`, the last drawn from the model the machine reports; `desktop.ini` carries
  `LocalizedResourceName=Samsung 64 GB` in UTF-16 with its mark.
- What that pair **does for the drive on this Windows is nothing**, and the reason is measured rather than
  assumed: while the pair stood, this Windows named the item `Samsung 64 (F:)` both in the *this PC* enumeration
  and as the name of the browsed folder — the volume's own label, which is also what `Get-Volume` answers. The pin
  needs the root's system attribute, measured 2026-11-18 on a plain folder, and **the root of this exFAT volume
  takes no such attribute**: `SetFileAttributesW` refuses the write — the .NET wrapper words it *Ungültiger
  Datei- oder Verzeichnisattributwert* — and `attrib +s +h F:\.` answers *file not found*. Whether a marked volume
  root would show a pinned name therefore cannot be settled on this machine at all; what is settled is that an
  unmarked root shows the label, and that the two files are written as the rule names them.
- The **first such run wrote `RootMarked=1` into both files while the root read as plain `Directory`**, because
  the code answered *the mark is absent, so the marking falls to us* instead of asking the machine whether the
  write took. That is a claim about the volume which the volume contradicts, and a later removal of that pair
  would have reported taking back a mark that never stood. The marker asks the machine now, and the same rename
  re-run after that fix writes `RootMarked=0` in both files with every other word identical.
- The taking back: a rename to nothing answered *the volume carries no name of its own. autorun.inf and
  desktop.ini of ours were taken back, since nothing of ours pins a longer name any more*, and neither file stood
  on the root afterwards. The volume then read as it had before the step: label empty per `Get-Volume`, exFAT,
  capacity 64 157 253 632, free 64 156 598 272, its root holding only `System Volume Information`. One wording
  trap belongs to this record: the enumeration of this feature answers the label of that unnamed volume as
  `Removable Disk` — the name Windows shows — where `Get-Volume` answers empty, so a claim of *back to the name it
  had* has to say which of those two answers it means.

Measured on that same stick on that same day for **the four files arriving with the bytes they carry** (README:
*Device icons*), which is what the user's report asked for: *create device icon* had left `DevIcon.fil` there
alone and **0 bytes**, with `folder.ico`, `autorun.inf` and `desktop.ini` absent. The act under test was the
product's own again — the scratch driver outside the repository calling `fsformat::act::create_icon` and
`act::remove_icon` against the real `F:` — and every number below is the volume's own answer read back, not the
act's words.

- What the failed act had left: one name, `DevIcon.fil`, **0 bytes**, carrying nothing but `Archive`, and no other
  of the four standing. The volume was healthy and quick in the same minute — `fsutil dirty query F:` answers
  *F: ist NICHT fehlerhaft*, a 5-byte write to another name took **12 ms** and a 408 966-byte write **25 ms**,
  both taken back afterwards — so that file was an act of ours that had opened the name and never filled it: that
  act's run had blocked in that write past 300 s and had to be ended. Whether a handle stood at the name while it
  blocked is **not** settled here; a later probe answered that the name opens exclusively, so whatever had stood
  was gone with the ended act.
- The taking back of a whole set: `unicon` answered *folder.ico, DevIcon.fil, autorun.inf, desktop.ini taken away
  from F:\; nothing of ours was there.*, the root reading `System Volume Information` afterwards. Before that run
  the four stood again with real bytes (408 966, 408 966, 220 and 298), which is what an act that completes
  writes; the 0-byte state above was the interrupted one.
- A staged name of ours goes with the four although no marked text file names it: `autorun.inf.fbtmp` with 19
  bytes standing answered *autorun.inf.fbtmp taken away from F:\; nothing of ours was there.*, root clean
  afterwards. The staged name is the file's own name with `.fbtmp` — **with its dot** — behind it: the first
  attempt of this probe left `autorun.inffbtmp` and the verb answered *Nothing taken away*, which is right, that
  name being nobody's of ours.
- The name another program holds: `F:\desktop.ini` was made a fixture of 25 bytes and held with **no sharing at
  all** while the create act ran. It answered `desktop.ini could not be put in place: Zugriff verweigert (os error
  5) (another program holds that name open)`. `DevIcon.fil`, `folder.ico` and `autorun.inf` read **absent**
  afterwards — the two pictures that had already arrived were taken back — no staged name stood, the fixture kept
  its 25 bytes, no consent prompt appeared and no elevation was asked for, and the whole act with the handle open
  took **362 ms**. The fixture was taken back and the root read `System Volume Information` only.
- Why that refusal cannot read its own code, measured on this machine: the **move** of a staged file onto a name
  held with no sharing answers **os error 5**, the very code a write to a read-only name answers and whose
  `ErrorKind` is `PermissionDenied` — trusting the code would send a held name to the administrator for a write
  no elevation can win. Writing straight to the name, which is what the baseline did, answers **os error 32**
  there (*Der Prozess kann nicht auf die Datei zugreifen, da sie von einem anderen Prozess verwendet wird*), and
  this toolchain maps that to **no** `denied` answer at all, so the baseline quoted the machine's words and named
  no reason. The refusal asks the machine instead: an open that permits no sharing answers the sharing conflict
  for as long as any handle of another program holds the name. Two assertions of the lib pin that pair of
  answers, and one of them pins it as a measurement — the same code for a held name and for a read-only name, and
  the open that sees the handle where the read-only name has none.
- The red of the held-name scenario at the baseline: `git archive HEAD` built into `C:\fbhead16` with only that
  scenario appended answers **FAILED** — *the refusal does not say what stood in the way: DevIcon.fil could not be
  written: … (os error 32)* — the report's second half. The other scenarios of this step cannot be built at that
  baseline at all, a staged name being no name that baseline knows, and one of them measures an answer code; they
  pin what this step added rather than a defect it fixes, and that is said here rather than dressed up as a red.
- The create of the fixed build on the real volume: *The device icon stands on F:\: DevIcon.fil, folder.ico,
  autorun.inf, desktop.ini.* — `DevIcon.fil` and `folder.ico` **408 966** bytes each with the container's own
  header answering **12** images and the marks `Hidden, Archive`, `autorun.inf` 220 bytes and `desktop.ini` 298
  bytes both `Hidden, System, Archive`, carrying `label=Samsung 64` beside `LocalizedResourceName=Samsung 64 GB`
  (the pin the pair is for: that name is 13 characters and the label of this exFAT volume takes 11, measured
  above), beside `ModelName=USB stick 64 GB`, `Manufacturer=Samsung`, `icon=folder.ico,0`,
  `IconResource=folder.ico,0`, `Image=DevIcon.fil` and `RootMarked=0`. Those four stand on the stick now, leaving
  them being what the verb was asked to do; one more run of *Remove device icon* takes them back. A census after
  every run of this step named no `failbrauwser`, no `fb-archive` and no driver of mine.

The gate of the same day, after the marker was fixed. `cargo test --release --lib` answers **406 passed, 3
failed, 1 ignored** — the three same Linux-only assertions named above. `fsformat::` alone answers **96 passed,
0 failed**. The four drives scenarios serial read **4 passed in 75.96 s**. `cargo clippy --release --lib --bins`
names **170** warnings with `src\fsformat` carrying none of them, and a forced rebuild of `--bin failbrauwser`
from the touched source names **no warning**, which is what `winpack\build.ps1` needs to get through its own
`$ErrorActionPreference = 'Stop'`. Of the guard targets, run in one batch with `--no-fail-fast`: `git_repo` 7,
`helper_lifetime` 2, `source_rules` 3 and `win_startup` 2, all green — after one assertion of `helper_lifetime`
had failed in an earlier such batch and read green in the two runs after it, alone and in the same batch again.
Why it failed there is not settled here; it is one of the two assertions that wait on a process this machine
schedules, no scenario of this feature failed with it, and it is recorded rather than hidden. The whole UI target of
that same tree, run serially rather than as four scenarios: **51 passed, 2 failed in 579.51 s**, the two failures
being `custom_actions` and `encrypted_drive_with_lvm_inside` — both named by this section for as long as it has
existed — while `archives_as_folders`, which failed in the 2026-11-17 shape, answered green here. A process census
after every run of that day named no `failbrauwser`, no `fb-archive` and no `format.com`, and the scratch driver
this section names was deleted with its probe scripts.

The gate of the same day again, after the four files began arriving in two moves. `cargo test --release --lib`
answers **410 passed, 3 failed, 1 ignored** — the three same Linux-only assertions named above — and
`fsformat::` alone **100 passed, 0 failed**, four scenarios more than the paragraph above. The four guard targets
in one batch with `--no-fail-fast`, each count read beside its own `Running tests\<name>.rs` line: `git_repo` 7,
`helper_lifetime` 2, `source_rules` 3 and `win_startup` 2, all green. `cargo clippy --release --lib --bins` names
**170** warnings, the same count the paragraph above records, with `src\fsformat` carrying **0** of them, and a
forced rebuild of `--bin failbrauwser` from the touched source names **no warning**.

The whole UI target of that tree, serial again: **44 passed, 9 failed in 369.06 s**. The nine are not this step's,
and the discriminator was measured on that same run: `Get-Clipboard` answers *ExternalException: Der angeforderte
Clipboard-Vorgang war nicht erfolgreich*, the condition this section already names for the clipboard/job group of
2026-11-17, where the same group read 44/8 and answered **0 green** re-run alone. Three of them re-run alone here
— `file_operations`, `archives_as_folders`, `checksums_in_properties`, the Release helper named — answered **0
passed, 3 failed in 67.59 s**, each stopping at a copy or a job: `wait-exists …\dest\a.txt` and `wait-row
new.txt` and `wait-properties-text ✔ Matches — the SHA-256 checksum`, each of them with `jobs busy false` where
the scenario expected work. None of the nine reaches the format, rename or icon path this step touched, and the
M16 scenario `drive_format` is not among the named failures, so it stands among the 44 that answered green. With
the clipboard answering, the same target of the same tree read **51 passed, 2 failed in 579.51 s** earlier that
same day, and those two — `custom_actions` and `encrypted_drive_with_lvm_inside` — are two of these nine. So the
seven extra belong to this host's clipboard state rather than to any tree, and the honest statement of the UI gate
here is that it rides on the clipboard answering: a run that reads 44/9 while `Get-Clipboard` refuses is that
condition, and it is recorded as one instead of being waited out. The driver and the probe scripts of this step
were deleted with it, and the baseline snapshot `C:\fbhead16` with them.

The **install of this build could not be put**, and the reason is this machine's answer rather than the package's:
`winpack\install.ps1` refused with *Der Prozess kann nicht auf die Datei …`bin\failbrauwser.exe` zugreifen, da sie
von einem anderen Prozess verwendet wird*, an instance of the installed image running at the time (pid 24128,
started 11:34:37, its own path the installed `bin\failbrauwser.exe`). Nothing there was left mixed by that
refusal, read back afterwards: the installed program is still the one the previous install wrote — 15 047 680
bytes, the first 16 hex digits of its SHA-256 `69CB5952A5B91B64`, written 10:35:38 — where this tree's fresh build
is 15 057 920 bytes beginning `0C32EE4D3FF04886`; the installed helper stands at its 68 039 168 bytes of 2026-11-18
untouched, the stamp there reads `1.0.0 20261119T104314`, which is that older image's own, and `bin` holds its 50
files with the loader cache at its 105 lines. The DLLs beside them are the same bytes either way, no native
dependency of this tree having changed since the install above. What was built and packed is `BUILD OK` with 47
DLLs staged, `winpack\out\failbrauwser-1.0.0-x86_64-windows.zip` at 57.2 MB and the launcher at 57.7 MB, so one
closed window is all that stands between that folder and this tree — and no instance the user was running was
ended to get this far, which is why the install is recorded as pending rather than done.

## Measured on 2026-11-19 for the listing reading those files back (README: *Device icons*, M16.7)

The user's ask was the other half of the verb: having written the four files, failBrauwser itself shows the
device by what they say. `fsformat::icon::shown(root)` answers that pair — the long name a marked `desktop.ini`
pins and the image a marked file names, our own `Image=` before the shell's `IconResource=` and `icon=` — and it
answers nothing at all for a text file without the section of ours. The answer rides on two new `Volume` fields
(`pinned_name`, `picture_file`), is filled in `ui::drives::gather` off the UI thread beside the fill levels, wins
`Volume::title()` ahead of the label, the mount point and the image file, and reaches the screen through one
helper (`ui::icons::volume_picture` / `volume_image`) that the drives page row, the side panel's drive row and a
volume's properties header all ask.

- **Red first, for the intended reason.** The six reading scenarios did not compile at this baseline —
  `cannot find function 'shown'`, `cannot find type 'Shown'`, and `missing fields 'picture_file' and
  'pinned_name'` — because the rule did not exist there. With the rule and the fields in and *nothing filling
  them*, the new scenario answered its real behavioural red: `device_shown.fbt` stopped at line 19 with
  `wait-drive Samsung 64 GB Family Pictures: timed out` while its first eighteen lines (both stand-in sticks,
  the unmounted name, `expect-drive-picture Samsung : none`, the mount, the location) answered green. Filling
  `gather` is what turned that line green: the scenario then ran in 3.64 s.
- **On the volume of this machine.** Through a throwaway crate driving the product's own `icon::shown` against
  `F:\` (created at `A:\wpdscratch\fb17read`, deleted with its probe scripts and run logs afterwards): the four
  files stand there at 219 and 300 bytes (attributes 6 = hidden|system) and at 408 966 bytes each for
  `folder.ico` and `DevIcon.fil` (attributes 2 = hidden), both text files marked, and their words are
  `label=Removable`, `LocalizedResourceName=Removable Disk`, `Image=DevIcon.fil`, `RootMarked=0` — which is the
  pin contract the user's own acts put there: fourteen characters pinned against the eleven this Windows
  honours for that exFAT root, whose own label therefore reads `Removable`. The read answers
  `pinned "Removable Disk"` and `picture "F:\DevIcon.fil"`, and a folder of this run holding none of the four
  answers empty for both, which is the volume's machine-given name and themed icon left alone.
- **What the payload's loader answers.** Decoding those two icon files through `Pixbuf::from_read` — the ICO
  loader this payload ships (`libpixbufloader-ico.dll`, its cache in the installed `loaders.cache` at 105 lines)
  — gives **256×256** for both: the loader takes the **largest** of the twelve images, and the row scales that
  down to the pixels its icon size resolves to. The number is pinned in
  `the_picture_of_a_written_icon_reaches_pixels_through_the_loader_this_payload_ships`, so a payload that ever
  loses that loader fails a test rather than quietly drawing themed icons.
- **The gates of this tree.** `cargo test --release --lib` answers **417 passed, 3 failed, 1 ignored** — the same
  three Linux-only assertions, and seven scenarios more than the 410 of the paragraph above. `fsformat::` alone
  **106 passed, 0 failed** and `drives::` **12 passed, 0 failed**. `helper_lifetime` 2, `source_rules` 3 and
  `win_startup` 2 in one batch, all green (no `git_repo` run this step: no repository code was touched).
  `cargo clippy --release --lib --bins` names **170** warnings — the same count as the two paragraphs above,
  reached after fixing the one warning this step's refactor of `named_files` introduced (a collapsible `if`) —
  with **0** of them in `src\fsformat`, and a forced rebuild of `--bin failbrauwser` from the six touched files
  names **no warning**.
- **The whole UI target, serial: 46 passed, 8 failed in 334.34 s**, with the discriminator measured again at the
  end of that run — `Get-Clipboard` answers `ExternalException`, the condition the section above names. The eight
  are that group and the two long-standing ones, named: `file_operations`, `archives_as_folders`,
  `checksums_in_properties`, `undo_operations`, `queue.fbt` run as `queued_paused_and_verified_copies`,
  `share_without_change_notices` (which runs `share_refresh.fbt`), plus `custom_actions` and
  `encrypted_drive_with_lvm_inside` — the last two fail even with the clipboard answering. `update` answered
  green this time, which the space-in-the-path paragraph has seen it do before. **The new scenario
  `a_device_shows_what_its_own_files_name` answered green in that same run**, driving the create's four files
  into the folder of a stand-in stick: name and picture appear when the stick is mounted and the volume names a
  folder, `expect-drive-picture … : DevIcon.fil` says the bytes reach pixels, the side panel names the stick by
  the same pinned name, a second stick whose folder holds nothing of ours answers `none`, and *Remove device
  icon* through the row menu brings the machine's own name and themed icon back. The run's own check read the
  fixture folder afterwards: none of the four names, none of the four staged names, and no mark of ours left on
  the folder.
- **Not verified here, and said so.** The side panel's picture and the properties header's picture are *not*
  asserted by any scenario — both call the one helper the drives page row asserts, and no scenario reads a
  `gtk::Image` of a row. The Linux half of this reading was never run (the reading code is platform-neutral, the
  fill is reached by both gather paths, and `network` volumes are skipped by that fill on either platform, so a
  GVfs share is not read). Coverage, mutation and branch gates stay unavailable — no instrumentation in this
  repository. And the **install of this build is pending**: two instances of the installed image were running
  when the gates finished (pid 24036 since 15:29:57 and pid 24128 since 11:34:37, both from
  `%LOCALAPPDATA%\Programs\failbrauwser\bin\failbrauwser.exe`), so `winpack\install.ps1` was not started rather
  than refused; no instance the user was running was ended, and the prefix still holds the image of the previous
  install with its own stamp. What the installed image shows for `F:` — whether it names it `Removable` or
  `Removable Disk` — therefore stays unmeasured until that install happens.

## Measured on 2026-11-20 for the two things the user asked after that (M16.8)

The report first: *the files stand on `F:` and nothing shows*. Measured rather than assumed, because the reading
had been proved on that volume the day before:

- **Which image the windows run.** Three instances were up — pid 24036 (since 19.11 15:29:57), pid 24128 (since
  19.11 11:34:37) and pid 32324 (**started 20.11 06:08:37**, a fresh window of the same old image) — all three
  from `%LOCALAPPDATA%\Programs\failbrauwser\bin\failbrauwser.exe`, whose bytes are
  `69CB5952A5B91B64…`, 15 047 680 long, stamped 19.11 10:35:38: the image of the **previous** install. The tree's
  own build is `240272FCF72D1644…`, 15 093 248 long. Nothing had reached the prefix, because the install had
  never been started for the M16.7 build.
- **What each image answers for the real stick.** One `.fbt` (`action win.drives`, `wait-drive Removable Disk`,
  `expect-drive-picture Removable Disk : DevIcon.fil`) driven through `tests\ui\harness.ps1` against the live
  machine, once per binary, work folders `A:\wpdscratch\fb20real\t` and `\ti` (space-free, as that section of
  this file requires), scratch script and folders deleted afterwards. The tree's build: **SELFTEST OK** — the
  picture is read from `F:\DevIcon.fil`, and it reaches pixels. The installed image:
  `SELFTEST FAIL (line 5): expect-drive-picture Removable Disk : DevIcon.fil: unknown command` — that verb is
  not in it at all, which is the proof of the stale prefix rather than of a broken reading.
- **A caution this machine cannot clear.** Line 4 (`wait-drive Removable Disk`) answered green **in both**
  binaries: for an unlabelled removable drive `win::storage` supplies the Explorer-style name
  `Removable Disk` (the fallback at `src\win\storage.rs:52` and `:65`), and this stick's pin reads exactly
  `Removable Disk` — so on `F:` the pinned name and the machine's fallback are the same string, and no scenario
  on this host can tell them apart by that name alone. The picture is what tells them apart, and it is what the
  report said was missing.

The ask that followed: *a broken file or image must not hack the process — undecodable, too large, or an ini/inf
doing strange things, and we stick to the default.* The bound, now held by the module rather than by luck, is in
the README paragraph this section's predecessor named; the numbers come from the artifacts of a real create
(219 and 300 bytes of text, 408 966 bytes each for the two icon files, largest image 256 pixels), each bound set
generously above its own measure: **64 KiB** of text, **8 MiB** and **256 pixels** of picture, **255 characters**
of pinned name. Reading stops at the bound instead of reading up to it, so a file's length decides no time and no
memory here; `pictures_are_ones_a_file_can_hold` reads only the directory and each image's own header — a header
naming more pixels than an icon directory can name, an entry placing or counting its image outside the file, an
image count with no room in the file, a cursor file, and a zero count are each refused — and `picture_bytes`
reads the bytes through the same open handle the answer came from, so a file cannot change shape between the two.
A pinned name that is empty, absurdly long, or carrying a control character or a **directional override**
(U+202A…U+202E, U+2066…U+2069: a control that would make a row read backwards from what it says) pins nothing,
and `named_here` now also refuses a picture name carrying a control character, ending in a dot, or longer than a
path component. Each answer falls back on its own: a nonsense name costs the name alone, a nonsense picture the
picture alone, and a named picture that fails the probe is replaced by the next one that same file names.

- **Red first, for the intended reason.** Seven new assertions on the pre-implementation tree, six of them
  behaviour reds using only the API that tree already had: `a name out of a file no reader of ours would finish
  was shown: "64GB USB"`; `a name longer than a path component was shown as a volume's name: "xxxx…"`;
  `a name carrying a control character was obeyed` (left `Some("Dev\u{1}Icon.fil")`);
  `a picture that is no icon was drawn anyway`; `an image header naming pixels no icon directory can name was
  drawn as a volume's picture`; `a text file past the bound was deleted as one of ours`. The 26 existing
  `fsformat::icon::tests` stayed green beside them, so the red was the new claim and not a broken fixture.
- **The bounds, green.** `fsformat::icon::` and `fsformat::ico::` together: **40 passed, 0 failed**. The probe
  accepts the honest one-image file and refuses the four lies in the same test, and the real twelve-image file
  this writer produces is accepted through `picture_named`, which is what keeps the guard from breaking the
  program's own icons.
- **The gates of this tree.** `fsformat::` and `drives::` together **125 passed, 0 failed**. The full lib
  target, run twice: **423 passed / 4 failed** and **422 passed / 5 failed** — the three recorded Linux-only
  assertions every time, plus extras that **change between runs** (`the_installed_tool_s_answer_stays_inside_the_table`
  and `a_client_still_reading_when_the_instance_ends_is_ended_there` in the first run,
  `querying_and_listing_agree_on_the_fixture` in the second), each of which answers **green when run alone**
  (0.24 s for the first two, 0.48 s for the third). This section records those three as **load-sensitive under
  the parallel lib run**, a class the file has named before for `big_listings.fbt`; nothing in this change
  touches a repository client, the Recycle Bin or the format table. `helper_lifetime` 2, `source_rules` 3,
  `win_startup` 2, all green in one batch. `cargo clippy --release --lib --bins` names **168** warnings
  (50 lib + 118 bin) with **0** in `src\fsformat\icon.rs`, `src\fsformat\ico.rs`, `src\ui\icons.rs` and the
  lines this step touched of `src\ui\selftest.rs`: the paragraph above recorded **170**, so the same command was
  run against the pre-implementation tree (`1f13bfe`, unpacked with `git archive` into `C:\fbhead`, built with
  this checkout's sysroot, deleted afterwards) and it answers **50 + 118** too — the recorded 170 belongs to the
  older toolchain, and this change adds no finding. One finding the new code did introduce along the way
  (`op_ref` on a slice comparison) was fixed before the commit. `--bin failbrauwser` rebuilt with no warning.
- **The whole UI target, serial: 52 passed, 2 failed in 245.51 s**, the clipboard answering at the start of the
  run (`Set-Clipboard`/`Get-Clipboard` round-tripping `fbprobe`) — which is the good condition this file names,
  and `share_without_change_notices` answered green this time. The two failures are exactly the two this section
  has named for as long as it has existed: `custom_actions` and `encrypted_drive_with_lvm_inside`.
- **The scenario grown, and what it now proves.** `a_device_shows_what_its_own_files_name` green in 4.52 s with a
  **third** stand-in stick whose pair of ours stands complete while `std::fs::write` puts a sentence of nonsense
  over its `folder.ico` and `DevIcon.fil`: the row is named by the pinned `Broken Stick With A Rotten Picture`
  — drives page and side panel both — and `expect-drive-picture … : none` says **nothing was read and nothing is
  drawn**, which is a tightened verb: `none` now requires `picture_file` to be absent, not merely an undecoded
  file. The scenario ends by taking that stick's icon away too, and the run's check read both fixture folders:
  none of the four names, none of the four staged names, and no mark of ours left on either folder.
- **Real volume, hardened build.** The same `.fbt` against the rebuilt binary over the live drives page answered
  **SELFTEST OK** again: the probe changes nothing about what a real device icon shows.
- **Not verified here, and said so.** No test hands a crafted decompression bomb to the installed payload's
  loader — deliberately doing that is the fault this change exists to prevent, and it is answered at the header
  probe instead, which never allocates the picture; the loader's own behaviour with a lying container stays
  unmeasured on purpose. The side panel's and the properties header's *pixels* stay unasserted (no scenario reads
  a `gtk::Image` off a row), as does the whole Linux half, and coverage, mutation and branch gates stay
  unavailable. The **install is still pending**: the three instances above hold
  `bin\failbrauwser.exe`, `winpack\install.ps1` copies into the prefix in place (`Copy-Item -Force`) and needs a
  package from `winpack\out`, which holds the **previous** build's zip and launcher — so starting it against a
  running instance risks a half-copied prefix, and it was not started. No instance the user was running was
  ended. Until the user closes those windows and `winpack\package.ps1` plus `install.ps1` have run, the installed
  image shows no pinned name and no drawn picture for any volume.
