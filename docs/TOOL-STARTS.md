# Every program failBrauwser starts

Audited 2026-11-17 across `src`, `helper`, `vendor`, `winpack` and `build.rs`. One program file,
two systems, one rule: a program this file manager starts in the background stays in the
background, and a window that does appear is one the user asked for. Read this before adding a
start — and before "fixing" a window that is meant to be there.

Line numbers are the state of 2026-11-17; they move. The rule that pins them is the one in
`tests/source_rules.rs`, which names the offending `file:line` when it goes red.

## Windows: why a console tool gets a window

`bin\failbrauwser.exe` is linked as a windowed program (pinned by `tests\win_startup.rs`), so it
has no console. Windows then gives every console-shaped child a console of its own — the black
window — for as long as it runs. `win\console::no_window` sets `CREATE_NO_WINDOW` (0x0800_0000,
its value pinned by the same test file) on the start, which keeps the console and drops the window.

The flag was measured on 2026-11-12 with the archive helper: started through a windowed parent it
appeared on the desktop as a console window titled with its own path; with the flag it did not
(`docs\WINDOWS.md`). The rule is exception-free — the launcher's own hand-over repeats the flag
literally (`src\portable.rs:46`, `:328`) because `winpack\onefile` builds that file without a win
module of its own.

Nothing here redirects the standard handles: a windowed program with no handles still has the C
runtime's, and a console-less start backs them with the console device.

## Linux: the same complaint looks different

A GUI process on Linux gets no terminal for its children, so there is no black window to remove.
The equivalent complaint there is a client that answers *by itself*: a prompt on the terminal, a
credential window of its own, or a pager standing between its words and the pipe reading them. The
same table closes both systems — `vcs::run::NON_INTERACTIVE` (`src\vcs\run.rs:41`):
`GIT_TERMINAL_PROMPT`, empty `GIT_PAGER`/`PAGER`, `HGPLAIN`, `GCM_INTERACTIVE=never`,
`SVN_INTERACTIVE=false`, empty `SSH_ASKPASS`. `ui::vcs::progress::prepare` puts this program's own
askpass on top of it, so a password question comes to a failBrauwser window instead of a helper's.

## The starts

| Start | Program | What for | On Windows | On Linux |
| --- | --- | --- | --- | --- |
| `src\archive\client.rs:223` | `fb-archive.exe` | archive listing and extraction (the C# helper) | guarded (`:226`), and the helper ends with the instance that started it | the same start, or `pkexec` through it |
| `src\vcs\git.rs:25` | `git` | every status, log, blame, diff and Repository verb | guarded (`:31`) — the one funnel all git traffic goes through | same code |
| `src\vcs\run.rs:60`, `:109`, `:145` | the client of the tool asked for (`git`, `svn`, `hg`, `cvs`, `ss`) | the machine-readable read, the plain call, and the streamed run with progress | guarded (`:67`, `:113`, `:148`) | same code |
| `src\vcs\cvs.rs:269` | `cvs` | the CVS-only acts (its own output shape) | guarded (`:273`) | same code |
| `src\ui\vcs\progress.rs:253` | the program the progress window was opened for | commit, push, merge, update… | guarded through `prepare` (`:115`) | same code |
| `src\ui\vcs\dialogs.rs:30` | `git` | quick acts (init, rename, delete) without a progress window | guarded through `prepare` (`:32`) | same code |
| `src\ui\vcs\generic.rs:347` | the tool's client | the patch a folder or a revision writes | guarded (`:351`) | same code |
| `src\vcs\mod.rs:344`, `:355`, `src\vcs\run.rs:192`, `:203` | a client's `--version` and its fixture acts | availability and scenario setup | guarded (`:347`, `:359`, `:195`, `:206`) | same code |
| `src\ui\update.rs:177` | `curl`, then `wget` | the GitHub release check | guarded (`:181`) | same code |
| `src\ui\update.rs:255` | `setsid sh update.sh` | the install that outlives this process | not built | detached session on purpose: the script lives on while the new version replaces this one |
| `src\ui\update.rs:260` | `powershell` | the file swap around a running image | guarded (`:261`) **and** `-WindowStyle Hidden` — a swap window would be a window nobody owns | not built |
| `src\update.rs:324`, `:332` | `powershell` | the update test's own package and swap | guarded (`:327`, `:335`) | not built |
| `src\ui\customactions.rs:81` | `cmd /C`, else `sh -c` | a custom action, including *Open Terminal Here* | guarded (`:85`); stdin nulled | the same start, and see below for why its window still appears |
| `src\links\helper.rs:115` | `pkexec failbrauwser --links …` | Backup Mode and mountpoints as root | guarded (`:127`) for the starts a Windows build can reach through `FAILBRAUWSER_ELEVATE`; the elevation run itself is the Unix path (`ui\links.rs:49`) | pkexec's own authentication dialog appears, which is the point |
| `src\ui\drives.rs:1071` | `pkexec failbrauwser --lvm …` | the LVM volumes inside an unlocked partition | never reached: `pkexec` is looked for first (`:1068`) and the answer is the "no pkexec" message | polkit's dialog appears, which is the point |
| `src\lvm.rs:259` | `lsblk`, `lvm`, `cryptsetup` … | the Linux drive facts, as root | `#[cfg(unix)]`: the whole half compiles nowhere else | no terminal is given to a child of a GUI process |
| `src\network.rs:184` | `smbclient -N -L` | the workgroup share probe | `#[cfg(not(windows))]`; the Windows build asks the network APIs | as left |
| `src\ui\network.rs:728` | `smbclient` | the browse list of a machine | guarded (`:733`) — this one does run on a Windows build | as left |
| `src\links\volume.rs:352` | `systemctl daemon-reload` | an `/etc/fstab` edit taking effect | guarded (`:355`) | as left |
| `src\ui\shares.rs:419` | `systemctl --user restart gvfs-daemon` | a share change being seen | guarded (`:422`) | as left |
| `src\ui\selftest.rs:639` | `sh -c …` | a scenario's `run` step | guarded (`:642`) | the scenario's words go to the harness log |
| `src\ui\selftest.rs:1356` | `grim` | a Wayland screenshot step | guarded (`:1359`); the step belongs to the compositor harness | as left |
| `src\fs\dirsize.rs:133` | `unshare -rm` | a test mounting inside itself to count once | `#[cfg(unix)]` test, guarded look-back | as left |
| `src\portable.rs:46` | the unpacked `failbrauwser.exe` | the single-file launcher handing over | `creation_flags(CREATE_NO_WINDOW)` written out (`:55`); this is the program's own file, so it changes no window | not built |
| `src\portable.rs:320` | `gdk-pixbuf-query-loaders.exe` | a loader cache with this machine's paths | written out (`:328`) | not built |

`helper\FbArchive` and the vendored CompressionWorkbench start no program at all (their codecs run
in process); only their own tests start `bsdtar` and friends. `build.rs` starts nothing, and no
`winpack\*.ps1` uses `Start-Process`.

## Windows that are the point

- **A terminal the user asked for.** *Open Terminal Here* runs `wt -d <folder>` or
  `start "" /d <folder> cmd /k` (`src\actions.rs:223`); the outer `cmd` is started without a window,
  and `start`/`wt` then open the one the user asked for. On Linux the desktop's terminal emulator is
  chosen instead. Do not "hide" this.
- **The program the user chose.** `gio::AppInfo` launches (`src\ui\util.rs:190`, `:203`,
  `src\ui\window.rs:532`, …) open the handler for a file type or URI: a window is the requested act.
- **The administrator's password.** pkexec's polkit dialog (rows above) is the only elevation
  prompt this program causes; it is never replaced by a window of ours.
- **A custom action's own program.** The action's command line is written for the shell named in
  `src\ui\customactions.rs:77`; its output goes where this program's output goes.

## What every start says

- **Whether it gets a window.** On Windows: `no_window`, `prepare`, or the flag written out.
  `tests\source_rules.rs` walks all of `src` and reports every `Command::new(` whose nearby lines
  carry none of them; a start the compiler only builds for Unix is exempt, because no window can
  appear there.
- **Which standard handles it is given.** Nothing started here is handed this program's own three
  streams, and it does not depend on each start remembering: `win::console::keep_our_handles_from_children`
  (`src\main.rs`) clears the inherit mark on them once, before anything is started. Windows marks the
  standard handles inheritable and duplicates every inheritable handle into a child, so without that
  each tool would hold a second copy of whatever started this program — a pipe a reader waits on for
  an end that never arrives, a console it can write into, or an input a CLI blocks on that nobody will
  type. What a start is given explicitly (`Stdio::piped()` for the answers that are read, `Stdio::null()`
  where a client must not read) is untouched by that mark; a start naming no stream falls back to the
  console device, which is what a windowed program has anyway. The single-file launcher keeps its own
  streams for the program it hands over, because that clearing happens inside the program being handed
  to, not inside the launcher.
- **What it may answer by itself.** The `NON_INTERACTIVE` table plus the caller's locale, and for
  the windows the askpass names in `progress::prepare`.
- **When it ends.** A *reading* tool does not outlive the instance that started it. The archive
  helper is told which process started it (`--parent`) and watches that process
  (`tests\helper_lifetime.rs`); the five clients are read by calls that wait for them —
  `run::ask`, `run::output`, `run::read_of` (for a caller that built its own `Command`) and the CVS
  and patch reads that go through it — and a read still in flight when this instance ends has its
  client ended there: `vcs::run::end_our_clients` (`src\main.rs`) walks the clients this instance
  registered and terminates each by id, and from that moment a client that starts is ended as soon
  as it is registered, so a read beginning during the shutdown cannot be left behind either. That
  covers the reads only, on purpose. An *act* the user
  asked for is left to finish — a commit (`run::with_input` feeds its message over a pipe, and its
  client is deliberately not registered), a push, a merge under its progress window, the installer
  script of an update detached on purpose (`src\ui\update.rs:255`) — and so is anything the user
  opened a window for: a terminal, a chosen handler, a custom action's own program.
  Measured here on 2026-11-17, before that rule existed: five tools were still running with their
  parent process gone when UI scenarios had ended — `git log -z --format=…`, two
  `git status --porcelain=v2 … -- .` and two `svn log --xml --verbose --revision 4 -- .`. They were
  invisible (`CREATE_NO_WINDOW`) and harmless-looking, at no CPU; what they cost is handles and
  pipes, and one of them is why a harness reading the harness's own pipes waited minutes for an end
  of file that never arrives (the test side reads a file now, `tests\ui_git.rs`).

## Kept by

| Check | What it pins |
| --- | --- |
| `tests\source_rules.rs::every_start_of_another_program_hides_its_console_window` | every `Command::new(` in `src` names its window answer |
| `tests\win_startup.rs` | the program file is windowed, and the flag's value is the API's |
| `tests\source_rules.rs::no_tree_iterators_changed_in_place` | unrelated: the row-stepping rule |
| `src\vcs\run.rs` unit test `the_client_environment_leaves_nothing_to_answer_on_its_own` | the table names each way the five clients prompt or page, and names no key twice |
| `src\win\console.rs` unit test `the_standard_handles_stay_this_programs_own` | the three standard streams of this program end up marked not inheritable |
| `src\vcs\run.rs` unit test `a_client_still_reading_when_the_instance_ends_is_ended_there` | a client of a read still in flight is gone when `end_our_clients` has run, and the call comes back refusing rather than hanging |

## Measured, and what is not

- Measured 2026-11-12: the archive helper as a console window, and its absence with the flag
  (`docs\WINDOWS.md`, `src\win\console.rs`).
- Measured 2026-11-17: the widened rule is load-bearing — a scratch file holding one unguarded
  `Command::new("git")` was reported as `src\audit_probe.rs:4`, and it went green again with the
  probe deleted. Before the widening it read only `src\vcs` and `src\ui\vcs`; the same probe went
  green through it, which is how the rest of `src` was found to need the rule.
- Not verified: no scenario watches the desktop while a Repository command runs, so "no black
  window" for a running log or commit rests on the flag value, the exception-free rule and the 2026
  -11-12 measurement — not on an observed desktop.
- Measured 2026-11-17, the inherited handle is not a theory: with the mark still on its standard
  handles this program leaves a second copy of them in what it starts. A Git UI scenario had
  answered `SELFTEST OK` into the win32 harness, the harness read two empty streams, the test
  wrapper waited on them for minutes with nothing printed, and one `git` from the run was still
  alive (one thread, `WaitReason=Suspended`, no CPU — it never ran). The same three handles are
  `Some(true)` for inheritable before the call and `Some(false)` after it, printed by
  `src\win\console.rs` unit test `the_standard_handles_stay_this_programs_own`; with the call body
  neutralized that test fails on exactly that assertion, which is how it was seen to be load-bearing.
  The scenario's own test wrapper now reads the harness through a file for the same reason
  (`tests\ui_git.rs`).
- Open: the reported start failures `0x800700E8` (Win32 232, a pipe already closed) for `git log`,
  `git ls-files` and the log-graph read name this program's own command lines. A handle inherited
  from whatever started failBrauwser is a plausible route to a start failing that way, and the rule
  above now cuts that route off at the source; it is not a proven explanation, because the reports
  came from an installed build that predates the per-start `Stdio::null()` and this clearing. That
  text is not produced by this repository's code (no such message and no translation catalog), so
  which program reported it is still open; reproduce it against a real working copy with the
  installed build before changing anything further for it.
