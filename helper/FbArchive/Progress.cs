using System.Text.Json;
using Compression.Registry;

namespace FbArchive;

/// <summary>
/// Progress of the request being handled, sent as extra lines before its answer:
/// <c>{"id":7,"progress":{"done":123,"total":456,"phase":"extracting"}}</c>.
/// A timer samples it a few times per second; nothing is sent for quick requests.
/// </summary>
public sealed class Progress : IDisposable {
  /// <summary>Guards stdout: progress lines and answers must not interleave.</summary>
  public static readonly object Out = new();
  public static Stream? Stdout;
  [ThreadStatic] private static Progress? _current;

  private readonly long _id;
  private readonly Timer _timer;
  private long _done;
  private long _total;
  private string _phase = "";
  private Func<long>? _poll;
  private long _lastSent = -1;

  private Progress(long id) {
    this._id = id;
    this._timer = new Timer(_ => this.Send(), null, 300, 250);
  }

  /// <summary>Starts reporting for request <paramref name="id"/>; dispose to stop.</summary>
  public static Progress Begin(long id) {
    var p = new Progress(id);
    _current = p;
    ArchiveInputInfo.ReadObserver = n => Interlocked.Add(ref p._done, n);
    return p;
  }

  /// <summary>The reporter of the request being handled, if any.</summary>
  public static Progress? Current => _current;

  /// <summary>Sets what is being done and how much there is.</summary>
  public void Phase(string phase, long total, Func<long>? poll = null) {
    this._phase = phase;
    Interlocked.Exchange(ref this._total, total);
    Interlocked.Exchange(ref this._done, 0);
    this._poll = poll;
  }

  private void Send() {
    // Nothing to report yet: a line without a phase is noise between the two sides.
    if (Stdout == null || this._phase.Length == 0) return;
    var done = this._poll?.Invoke() ?? Interlocked.Read(ref this._done);
    var total = Interlocked.Read(ref this._total);
    if (done == this._lastSent) return;
    this._lastSent = done;
    using var buffer = new MemoryStream();
    using (var w = new Utf8JsonWriter(buffer)) {
      w.WriteStartObject();
      w.WriteNumber("id", this._id);
      w.WriteStartObject("progress");
      w.WriteNumber("done", Math.Min(done, total > 0 ? total : done));
      w.WriteNumber("total", total);
      // Everything read, the writer still compressing: say so rather than "100 %".
      w.WriteString("phase", this._phase == "adding" && total > 0 && done >= total ? "writing" : this._phase);
      w.WriteEndObject();
      w.WriteEndObject();
    }
    lock (Out) {
      Stdout.Write(buffer.ToArray());
      Stdout.WriteByte((byte)'\n');
      Stdout.Flush();
    }
  }

  public void Dispose() {
    this._timer.Dispose();
    ArchiveInputInfo.ReadObserver = null;
    if (_current == this) _current = null;
  }

  /// <summary>Bytes in the files below <paramref name="dir"/> (to follow an extraction).</summary>
  public static long BytesBelow(string dir) {
    try {
      var o = new EnumerationOptions { RecurseSubdirectories = true, IgnoreInaccessible = true, AttributesToSkip = 0 };
      return new DirectoryInfo(dir).EnumerateFiles("*", o).Sum(f => f.Length);
    }
    catch (IOException) { return 0; }
    catch (UnauthorizedAccessException) { return 0; }
  }
}
