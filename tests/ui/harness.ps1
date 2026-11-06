# Runs failBrauwser with a self-test script on the win32 GDK surface (no compositor needed).
#   harness.ps1 <binary> <script> <workdir>
# The script may use $T for <workdir> and $UI for this folder. Config and data live below
# <workdir>, so the user's settings are never touched; the GTK theme settings are copied over.
# Windows twin of harness.sh: same contract, same output, same exit codes.
param([string]$BIN, [string]$SCRIPT, [string]$T)
$ErrorActionPreference = 'Stop'
# Parallel runs would pick up each other's private bus: one at a time.
$lock = $null
$deadline = (Get-Date).AddMinutes(5)
while ($true) {
    try { $lock = [IO.File]::Open((Join-Path $env:TEMP 'failbrauwser-ui-test.lock'), 'OpenOrCreate', 'ReadWrite', 'None'); break }
    catch { if ((Get-Date) -gt $deadline) { "UI test lock held for 5 minutes" | Out-Host; exit 1 }; Start-Sleep -Milliseconds 100 }
}
foreach ($d in 'config', 'data', 'cache') { New-Item -ItemType Directory -Force -Path (Join-Path $T $d) | Out-Null }
# The look comes from the user's desktop (even when the folders point at demo data);
# on win32 GTK the per-user folder is %APPDATA%. dconf and the tileWin feed have no
# win32 twin: the theme falls back to the sysroot default, the chart to the app palette.
$TH = if ($env:FB_THEME_HOME) { $env:FB_THEME_HOME } else { $env:APPDATA }
if (Test-Path (Join-Path $TH 'gtk-3.0')) { Copy-Item -Recurse -Force (Join-Path $TH 'gtk-3.0') (Join-Path $T 'config\gtk-3.0') }

# $T: the work folder; $UI: this folder (helpers next to the scripts);
# $TROOT: the root of the volume $T sits on — the tree and path bar root for a Windows path.
$UI = Split-Path -Parent $MyInvocation.MyCommand.Path
# $scriptFbt (never $script: PowerShell is case-insensitive and that would clobber $SCRIPT).
$scriptFbt = Join-Path $T 'script.fbt'
[IO.File]::WriteAllText($scriptFbt, [IO.File]::ReadAllText($SCRIPT).Replace('$TROOT', (Split-Path -Qualifier $T) + '\').Replace('$T', $T).Replace('$UI', $UI))

# The same isolation as the Linux harness: the app's config, data and cache stay below $T.
$env:XDG_CONFIG_HOME = Join-Path $T 'config'
$env:XDG_DATA_HOME = Join-Path $T 'data'
$env:XDG_CACHE_HOME = Join-Path $T 'cache'
# Scenarios name stock buttons in English (as the Linux runners' C locale does);
# without this GTK would answer in the Windows UI language ("Schließen").
$env:LANG = 'C'
$env:LC_ALL = 'C'
$env:FB_SELFTEST = $scriptFbt
# GLib-on-win32 starts its own session bus when the application asks for one (M0a); the
# private bus belongs to this run.
$out = Join-Path $T 'harness.out'
$err = Join-Path $T 'harness.err'
# .NET Process rather than Start-Process: on policy-managed machines a Start-Process
# object started without -Wait stops being able to read the child's exit code at all
# (even cmd.exe comes back blank), which would turn a killed run into a false zero.
$psi = [System.Diagnostics.ProcessStartInfo]::new()
$psi.FileName = $BIN
$psi.WorkingDirectory = $UI
$psi.UseShellExecute = $false
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$p = [System.Diagnostics.Process]::Start($psi)
$outRead = $p.StandardOutput.ReadToEndAsync()
$errRead = $p.StandardError.ReadToEndAsync()
$rc = 124
if ($p.WaitForExit(60000)) { $rc = $p.ExitCode } else { & taskkill /PID $p.Id /T /F | Out-Null; Start-Sleep -Milliseconds 500 }
$lock.Close()
$outText = if ($outRead.Wait(5000)) { $outRead.Result } else { '' }
$errText = if ($errRead.Wait(5000)) { $errRead.Result } else { '' }
[IO.File]::WriteAllText($out, $outText)
[IO.File]::WriteAllText($err, $errText)
# Same noise filter as harness.sh.
foreach ($line in ($outText + "`n" + $errText) -split "`r?`n") {
    if ($line -notmatch 'dbus-daemon|SpiRegistry|fusermount') { $line | Out-Host }
}
exit $rc
