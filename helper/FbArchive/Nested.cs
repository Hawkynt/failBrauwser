using Compression.Lib;
using Compression.Registry;
using F = Compression.Lib.FormatDetector.Format;

namespace FbArchive;

/// <summary>One step into an archive inside an archive: the entry, and the password of the
/// archive holding it.</summary>
public readonly record struct Level(string Entry, string? Password);

/// <summary>
/// Archives inside archives, read where they are: each level is opened as a stream inside the one
/// holding it, never written out. An encrypted volume (VeraCrypt, LUKS, BitLocker, dm-crypt) is
/// decrypted block by block as it is read; a filesystem image's file is read through its own
/// clusters. Only an entry that can be read front to back alone (a compressed one) is held in
/// memory, up to <see cref="MemoryLimit"/>. Nothing is written to any temporary folder while a
/// nested archive is read (<see cref="NoTempFiles"/>).
/// </summary>
public sealed class Nested : IDisposable {
  /// <summary>The innermost archive as a seekable stream.</summary>
  public Stream Stream { get; }
  /// <summary>Its format.</summary>
  public F Format { get; }
  /// <summary>Its name (the last level's entry name).</summary>
  public string Name { get; }
  /// <summary>Why it cannot be changed, when it cannot.</summary>
  public string? ReadOnlyReason { get; }

  private readonly List<IDisposable> _owned;

  private Nested(Stream stream, F format, string name, string? readOnlyReason, List<IDisposable> owned) {
    Stream = stream;
    Format = format;
    Name = name;
    ReadOnlyReason = readOnlyReason;
    _owned = owned;
  }

  public void Dispose() {
    for (var i = _owned.Count - 1; i >= 0; --i) _owned[i].Dispose();
  }

  /// <summary>The largest entry held in memory because it can only be read front to back.</summary>
  public static long MemoryLimit {
    get {
      var available = GC.GetGCMemoryInfo().TotalAvailableMemoryBytes;
      var quarter = available > 0 ? available / 4 : 1L << 30;
      return Math.Min(quarter, Array.MaxLength);
    }
  }

  /// <summary>
  /// Opens <paramref name="levels"/> one inside the other, starting at the file
  /// <paramref name="archive"/> (read as <paramref name="force"/> when given).
  /// </summary>
  public static Nested Open(string archive, string? force, IReadOnlyList<Level> levels) {
    if (levels.Count == 0) throw new ProtocolException("no nested level given");
    var owned = new List<IDisposable>();
    try {
      Stream current;
      F format;
      string? readOnly = null;
      string name = Path.GetFileName(archive);
      if (force != null) {
        current = ArchiveVolumes.OpenRead(archive);
        format = Enum.TryParse<F>(force, ignoreCase: true, out var f) ? f : throw new NotSupportedException($"No such format '{force}'.");
        readOnly = "a chosen Open-as container opens read-only";
      } else if (SplitSet.Parts(archive) is { } parts) {
        current = SplitSet.Open(parts);
        format = SplitSet.Format(parts[0], current);
        readOnly = SplitSet.ReadOnlyReason;
      } else {
        current = ArchiveVolumes.OpenRead(archive);
        format = ArchiveVolumes.Detect(archive);
        if (format == F.Sfx) {
          var sfx = FormatDetector.GetSfxArchiveInfo(archive) ?? throw new InvalidOperationException("SFX file contains no detectable archive.");
          owned.Add(current);
          current = new SubStream(current, sfx.Offset, sfx.Length);
          format = sfx.ArchiveFormat;
          readOnly = "an archive inside a self-extracting program opens read-only";
        }
      }
      owned.Add(current);
      foreach (var level in levels) {
        var (entry, encrypted) = OpenEntry(current, format, name, level);
        owned.Add(entry);
        if (encrypted) readOnly ??= "Inside an encrypted container, archives open read-only.";
        else if (level.Password != null) readOnly ??= "Inside a password-protected archive, archives open read-only.";
        else if (!Commands.IsWritable(format)) readOnly ??= $"It lies inside a {format} archive, which cannot be changed.";
        name = Commands.Normalize(level.Entry);
        current = entry;
        format = Detect(name, current);
        if (format == F.Unknown) throw new NotSupportedException($"“{name}” is not an archive that can be opened as a folder.");
      }
      return new Nested(current, format, name, readOnly, owned);
    } catch {
      for (var i = owned.Count - 1; i >= 0; --i) owned[i].Dispose();
      throw;
    }
  }

  /// <summary>What VeraCrypt, LUKS, BitLocker and dm-crypt name a volume's decrypted contents.</summary>
  public const string EncryptedVolumeEntry = "volume.img";

  /// <summary>The entry <paramref name="level"/> names inside <paramref name="container"/>, as a
  /// seekable stream, and whether it was decrypted to be read.</summary>
  private static (Stream Entry, bool Encrypted) OpenEntry(Stream container, F format, string containerName, Level level) {
    var wanted = Commands.Normalize(level.Entry);
    var ops = FormatRegistry.GetArchiveOps(format.ToString());
    if (ops == null) {
      // A compressed single file (.gz, .xz, …): its one entry is the decompressed contents.
      if (!FormatDetector.IsStreamFormat(format) || FormatRegistry.GetStreamOps(format.ToString()) is not { } streamOps)
        throw new NotSupportedException($"Cannot read inside a {format} file.");
      container.Position = 0;
      return (InMemory(wanted, output => streamOps.Decompress(container, output)), false);
    }
    // An encrypted volume: its decrypted view, read block by block, nothing held or written.
    // (Every encrypted volume format names its decrypted view so; asking its listing first would
    // run the slow key derivation twice.)
    if (ops is IEncryptedBlockDeviceProvider crypt && wanted == EncryptedVolumeEntry) {
      container.Position = 0;
      var device = crypt.OpenBlockDevice(container, new EncryptedVolumeCredentials(level.Password), writable: false, leaveOpen: true);
      return (new BlockDeviceStream(device, leaveOpen: false), true);
    }
    container.Position = 0;
    var raw = ops.List(container, level.Password).FirstOrDefault(e => !e.IsDirectory && Commands.Normalize(e.Name) == wanted)?.Name
      ?? throw new FileNotFoundException($"No such entry in {containerName}: {wanted}");
    container.Position = 0;
    using (NoTempFiles.Scope()) {
      Stream entry;
      try {
        entry = ops.OpenEntry(container, raw, level.Password);
      } catch (Exception e) when (NoTempFiles.WasRefused(e)) {
        throw new NotSupportedException($"“{wanted}” cannot be read inside {containerName} without unpacking it, so it is not opened. Extract it to open it.");
      }
      if (entry.CanSeek) return (entry, false);
      using (entry) return (InMemory(wanted, output => entry.CopyTo(output)), false);
    }
  }

  /// <summary>An entry that can only be read front to back, held in memory (never on disk).</summary>
  private static Stream InMemory(string name, Action<Stream> fill) {
    var limit = MemoryLimit;
    var memory = new LimitedMemoryStream(limit, name);
    fill(memory);
    memory.Position = 0;
    return memory;
  }

  /// <summary>
  /// The format of an entry, from its name and its contents: a name that says what it is decides,
  /// except behind a suffix that says nothing (".img", ".bin", …), where the contents do.
  /// </summary>
  public static F Detect(string name, Stream stream) {
    var byName = FormatDetector.DetectByExtension(name);
    var generic = GenericSuffixes.Contains(Path.GetExtension(name));
    if (byName != F.Unknown && !generic) return byName;
    var byContent = FormatDetector.DetectByContent(stream);
    return byContent != F.Unknown ? byContent : byName;
  }

  private static readonly HashSet<string> GenericSuffixes = new(StringComparer.OrdinalIgnoreCase) {
    "", ".bin", ".dat", ".img", ".dsk", ".raw", ".data", ".dump", ".out", ".image",
  };

  /// <summary>A memory stream that refuses to grow past a limit, naming the entry.</summary>
  private sealed class LimitedMemoryStream(long limit, string name) : MemoryStream {
    public override void Write(byte[] buffer, int offset, int count) {
      Check(count);
      base.Write(buffer, offset, count);
    }
    public override void Write(ReadOnlySpan<byte> buffer) {
      Check(buffer.Length);
      base.Write(buffer);
    }
    public override void WriteByte(byte value) {
      Check(1);
      base.WriteByte(value);
    }
    private void Check(int more) {
      if (this.Position + more > limit)
        throw new NotSupportedException($"“{name}” is compressed and larger than {limit / (1024 * 1024)} MiB, too large to be read inside the archive without unpacking it. Extract it to open it.");
    }
  }
}

/// <summary>
/// While a nested archive is read, the temporary folder points at a place no folder can be made
/// in, so no reader can fall back to writing an entry out (the library's generic per-entry reader
/// extracts to a temporary folder): it fails instead, and the user is told.
/// </summary>
public static class NoTempFiles {
  private const string Marker = "fb-archive-no-temporary-files";
  private static readonly string[] Variables = OperatingSystem.IsWindows() ? ["TMP", "TEMP"] : ["TMPDIR"];

  private static string Nowhere => OperatingSystem.IsWindows() ? $@"\\.\NUL\{Marker}" : $"/dev/null/{Marker}";

  public static IDisposable Scope() {
    var saved = Variables.Select(v => (v, Environment.GetEnvironmentVariable(v))).ToArray();
    foreach (var v in Variables) Environment.SetEnvironmentVariable(v, Nowhere);
    return new Restore(saved);
  }

  /// <summary>Whether <paramref name="e"/> came from a reader trying to write a temporary file.</summary>
  public static bool WasRefused(Exception e) {
    for (Exception? x = e; x != null; x = x.InnerException)
      if (x.Message.Contains(Marker, StringComparison.Ordinal) || x is IOException or UnauthorizedAccessException && Path.GetTempPath().Contains(Marker, StringComparison.Ordinal))
        return true;
    return false;
  }

  private sealed class Restore((string Name, string? Value)[] saved) : IDisposable {
    public void Dispose() {
      foreach (var (name, value) in saved) Environment.SetEnvironmentVariable(name, value);
    }
  }
}
