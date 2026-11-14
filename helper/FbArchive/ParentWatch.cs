using System.Runtime.InteropServices;
using System.Runtime.Versioning;

namespace FbArchive;

/// <summary>
/// The helper ends with the instance that started it: failBrauwser names its own process id as
/// <c>--parent &lt;pid&gt;</c> and this watches that process. Watching the process rather than
/// only the input pipe is what leaves no <c>fb-archive</c> behind when an instance is killed or
/// crashes, or when another child still holds the write end of the pipe open.
/// </summary>
public static class ParentWatch {
  /// <summary>Watches the instance named on the command line, if one was named, and ends this
  /// process as soon as that one is gone.</summary>
  public static void StartIfAsked(string[] args) {
    if (!TryParentId(args, out var pid)) return;
    new Thread(() => WaitThenExit(pid)) { IsBackground = true, Name = "parent-watch" }.Start();
  }

  private static bool TryParentId(string[] args, out uint pid) {
    var at = Array.IndexOf(args, "--parent");
    pid = 0;
    return at >= 0 && at + 1 < args.Length && uint.TryParse(args[at + 1], out pid);
  }

  /// <summary>Waits for the process to go away, then ends this one — a request still running
  /// included, which is the point: an instance that is gone has nobody left to answer.</summary>
  private static void WaitThenExit(uint pid) {
    if (OperatingSystem.IsWindows()) {
      var handle = NativeMethods.OpenProcess(NativeMethods.SYNCHRONIZE, false, pid);
      if (handle == IntPtr.Zero) Environment.Exit(0);   // the instance ended before we looked
      NativeMethods.WaitForSingleObject(handle, NativeMethods.INFINITE);
    } else {
      while (true) {
        try {
          if (System.Diagnostics.Process.GetProcessById((int)pid).HasExited) break;
        } catch (ArgumentException) {
          break;   // no such process: the instance is gone
        }
        Thread.Sleep(500);
      }
    }
    Environment.Exit(0);
  }

  private static class NativeMethods {
    public const uint SYNCHRONIZE = 0x0010_0010;
    public const uint INFINITE = 0xFFFF_FFFF;
    [SupportedOSPlatform("windows")] [DllImport("kernel32.dll")] public static extern IntPtr OpenProcess(uint access, bool inheritHandle, uint pid);
    [SupportedOSPlatform("windows")] [DllImport("kernel32.dll")] public static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
  }
}
