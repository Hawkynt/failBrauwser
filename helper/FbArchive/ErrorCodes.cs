using System.Text;

namespace FbArchive;

/// <summary>
/// The protocol's error classes and the sentence a user reads for them. The class drives what the
/// program does next (a password is asked for, an unsupported format is not retried); the sentence
/// has to say what happened in plain terms, since the library's own text arrives as a resource
/// string that no user reads (<c>IO_SharingViolation_File, D:\volume.hc</c>).
/// </summary>
public static class ErrorCodes {
  /// ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION: another program's handle answers, not the file.
  private const int SharingViolation = unchecked((int)0x80070020);
  private const int LockViolation = unchecked((int)0x80070021);
  /// Linux EWOULDBLOCK (= EAGAIN): .NET's flock emulation of FileShare.None refused the open
  /// because another handle holds the lock (measured: IOException with HResult 11).
  private const int LinuxWouldBlock = 11;

  public static string Code(Exception ex) {
    if (Hold(ex) is not null) return "inuse";
    for (var e = ex; e != null; e = e.InnerException) {
      var msg = e.Message;
      if (msg.Contains("password", StringComparison.OrdinalIgnoreCase) || msg.Contains("encrypt", StringComparison.OrdinalIgnoreCase))
        return "password";
      if (e is NotSupportedException) return "unsupported";
      if (e is FileNotFoundException or DirectoryNotFoundException) return "notfound";
      if (e is UnauthorizedAccessException) return "permission";
      if (e is ProtocolException) return "protocol";
    }
    return "error";
  }

  public static string Message(Exception ex) {
    if (Hold(ex) is { } hold) return InUse(hold);
    var sb = new StringBuilder(ex.Message);
    for (var e = ex.InnerException; e != null; e = e.InnerException) sb.Append(": ").Append(e.Message);
    return sb.ToString();
  }

  /// The exception whose system error code names another program's handle as the reason: a held
  /// volume outranks any wording, so a path that reads like a password question stays a holding one.
  private static IOException? Hold(Exception ex) {
    for (var e = ex; e != null; e = e.InnerException)
      if (e is IOException io && (io.HResult == SharingViolation || io.HResult == LockViolation || (OperatingSystem.IsLinux() && io.HResult == LinuxWouldBlock))) return io;
    return null;
  }

  /// The sentence for a held file: the library's own text is a resource string (<c>IO_SharingViolation_File,
  /// D:\volume.hc</c>) that says nothing to a user, and the path is the only part worth keeping.
  private static string InUse(IOException hold) {
    var msg = hold.Message;
    var (first, last) = (msg.IndexOf('\''), msg.LastIndexOf('\''));
    var comma = msg.IndexOf(", ", StringComparison.Ordinal);
    var path = first >= 0 && last > first + 1 ? msg[(first + 1)..last] : comma > 0 && comma + 2 < msg.Length ? msg[(comma + 2)..] : "";
    return $"{(path.Length > 0 ? path + " is" : "The file is")} in use by another program, which keeps it closed to other readers. Close or dismount it there first.";
  }
}
