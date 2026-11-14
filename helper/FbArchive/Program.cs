using System.Text;
using System.Text.Json;
using Compression.Lib;

namespace FbArchive;

/// <summary>
/// Line based JSON protocol. Every request is one JSON object on one line of stdin and gets
/// exactly one JSON object on one line of stdout. Requests are handled strictly in order.
/// <c>--version</c> answers the protocol version and leaves; <c>--parent &lt;pid&gt;</c> names the
/// instance this helper ends with (see <see cref="ParentWatch"/>).
/// </summary>
public static class Program {
  public const int ProtocolVersion = 1;

  public static int Main(string[] args) {
    if (args.Length > 0 && args[0] == "--version") {
      Console.WriteLine($"fb-archive protocol {ProtocolVersion}");
      return 0;
    }

    ParentWatch.StartIfAsked(args);
    var stdin = new StreamReader(Console.OpenStandardInput(), new UTF8Encoding(false));
    var stdout = Console.OpenStandardOutput();
    Progress.Stdout = stdout;
    string? line;
    while ((line = stdin.ReadLine()) != null) {
      if (line.Length == 0) continue;
      var response = Handle(line, out var quit);
      lock (Progress.Out) {
        stdout.Write(response);
        stdout.WriteByte((byte)'\n');
        stdout.Flush();
      }
      if (quit) break;
    }
    return 0;
  }

  /// <summary>Handles one request line and returns the UTF-8 encoded response line.</summary>
  public static byte[] Handle(string line, out bool quit) {
    quit = false;
    long id = 0;
    using var buffer = new MemoryStream();
    using (var w = new Utf8JsonWriter(buffer)) {
      try {
        using var doc = JsonDocument.Parse(line);
        var req = doc.RootElement;
        id = req.TryGetProperty("id", out var idEl) && idEl.ValueKind == JsonValueKind.Number ? idEl.GetInt64() : 0;
        var cmd = req.GetProperty("cmd").GetString() ?? "";
        using var progress = Progress.Begin(id);
        w.WriteStartObject();
        w.WriteNumber("id", id);
        w.WriteBoolean("ok", true);
        switch (cmd) {
          case "hello": w.WriteNumber("version", ProtocolVersion); break;
          case "quit": quit = true; break;
          case "formats": Commands.Formats(w); break;
          case "probe": Commands.Probe(w, Str(req, "path")); break;
          case "list": Commands.List(w, Str(req, "archive"), OptStr(req, "password"), OptStr(req, "format")); break;
          case "extract":
            Commands.Extract(Str(req, "archive"), Str(req, "dest"), OptStrArray(req, "entries"), OptStr(req, "password"), OptStr(req, "format"));
            break;
          case "options": Commands.Options(w, Str(req, "format")); break;
          case "add": Commands.Add(Str(req, "archive"), Items(req), OptStr(req, "password"), OptStr(req, "format"), Settings(req)); break;
          case "remove": Commands.Remove(Str(req, "archive"), OptStrArray(req, "names") ?? [], OptStr(req, "password")); break;
          case "rename": Commands.Rename(Str(req, "archive"), Str(req, "from"), Str(req, "to"), OptStr(req, "password")); break;
          case "mkdir": Commands.Mkdir(Str(req, "archive"), Str(req, "name"), OptStr(req, "password")); break;
          default: throw new ProtocolException($"unknown command '{cmd}'");
        }
        w.WriteEndObject();
      }
      catch (Exception ex) {
        w.Reset();
        buffer.SetLength(0);
        w.WriteStartObject();
        w.WriteNumber("id", id);
        w.WriteBoolean("ok", false);
        w.WriteString("code", ErrorCodes.Code(ex));
        w.WriteString("error", ErrorCodes.Message(ex));
        w.WriteEndObject();
      }
    }
    return buffer.ToArray();
  }

  private static string Str(JsonElement req, string name)
    => req.TryGetProperty(name, out var el) && el.ValueKind == JsonValueKind.String
      ? el.GetString()!
      : throw new ProtocolException($"missing string field '{name}'");

  private static string? OptStr(JsonElement req, string name)
    => req.TryGetProperty(name, out var el) && el.ValueKind == JsonValueKind.String ? el.GetString() : null;

  private static string[]? OptStrArray(JsonElement req, string name) {
    if (!req.TryGetProperty(name, out var el) || el.ValueKind != JsonValueKind.Array) return null;
    return el.EnumerateArray().Select(e => e.GetString() ?? "").ToArray();
  }

  /// <summary>How a new archive is written ("settings" object), if given.</summary>
  private static CreateSettings? Settings(JsonElement req) {
    if (!req.TryGetProperty("settings", out var s) || s.ValueKind != JsonValueKind.Object) return null;
    int? Int(string name) => s.TryGetProperty(name, out var e) && e.ValueKind == JsonValueKind.Number ? e.GetInt32() : null;
    long Long(string name) => s.TryGetProperty(name, out var e) && e.ValueKind == JsonValueKind.Number ? e.GetInt64() : 0;
    bool Bool(string name) => s.TryGetProperty(name, out var e) && e.ValueKind == JsonValueKind.True;
    Dictionary<string, string>? specific = null;
    if (s.TryGetProperty("specific", out var sp) && sp.ValueKind == JsonValueKind.Object)
      specific = sp.EnumerateObject().ToDictionary(p => p.Name, p => p.Value.ValueKind == JsonValueKind.String ? p.Value.GetString()! : p.Value.ToString());
    return new CreateSettings(OptStr(s, "method"), Int("level"), Long("dict_size"), Long("solid_size"), Int("threads") ?? 1,
      Bool("force_compress"), Bool("encrypt_names"), OptStr(s, "zip_encryption"), specific);
  }

  private static List<AddItem> Items(JsonElement req) {
    if (!req.TryGetProperty("items", out var el) || el.ValueKind != JsonValueKind.Array)
      throw new ProtocolException("missing array field 'items'");
    return el.EnumerateArray().Select(i => new AddItem(OptStr(i, "src") ?? "", Str(i, "name"))).ToList();
  }
}

public sealed class ProtocolException(string message) : Exception(message);

/// <summary>A file or directory on disk (<see cref="Src"/>) to be stored under <see cref="Name"/>.
/// An empty <see cref="Src"/> with a name ending in '/' creates an empty directory.</summary>
public readonly record struct AddItem(string Src, string Name);

/// <summary>How a new archive is written: method (empty: the format's own), level 0–9,
/// dictionary and solid block size in bytes (0: the format's default), threads, whether files
/// that do not shrink are compressed anyway, whether names are encrypted too (7z, RAR 5), the
/// ZIP encryption ("aes256", "zipcrypto") and the format's own settings by key.</summary>
public sealed record CreateSettings(string? Method, int? Level, long DictSize, long SolidSize, int Threads,
  bool ForceCompress, bool EncryptNames, string? ZipEncryption, IReadOnlyDictionary<string, string>? Specific);
