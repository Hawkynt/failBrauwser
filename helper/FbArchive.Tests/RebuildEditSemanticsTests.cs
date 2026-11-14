#pragma warning disable CS1591
using Compression.Lib;
using Compression.Registry;

namespace FbArchive.Tests;

/// <summary>
/// What an entry name means to Add and Remove when the edit goes through a rebuild:
/// exact, case-sensitive paths; a folder name covers its subtree; folders and times survive.
/// </summary>
[TestFixture]
public class RebuildEditSemanticsTests {

  private string _dir = null!;

  [SetUp]
  public void SetUp() => this._dir = Directory.CreateTempSubdirectory("cwb_semantics_").FullName;

  [TearDown]
  public void TearDown() { try { Directory.Delete(this._dir, true); } catch (IOException) { } }

  /// <summary>The edit-normal form of an entry name: forward slashes, no root prefix, no trailing slash.</summary>
  private static string Normalize(string entryName) {
    var s = entryName.Replace('\\', '/');
    while (true) {
      if (s.StartsWith("./", StringComparison.Ordinal)) s = s[2..];
      else if (s.StartsWith('/')) s = s[1..];
      else break;
    }
    while (s.Contains("//", StringComparison.Ordinal)) s = s.Replace("//", "/", StringComparison.Ordinal);
    return s.TrimEnd('/');
  }

  private string MakeTarGz() {
    var src = Path.Combine(this._dir, "src");
    Directory.CreateDirectory(Path.Combine(src, "sub"));
    File.WriteAllText(Path.Combine(src, "a.txt"), "root");
    File.WriteAllText(Path.Combine(src, "sub", "a.txt"), "nested");
    File.WriteAllText(Path.Combine(src, "sub", "b.txt"), "other");
    foreach (var f in Directory.GetFiles(src, "*", SearchOption.AllDirectories))
      File.SetLastWriteTimeUtc(f, new DateTime(2024, 1, 2, 3, 4, 5, DateTimeKind.Utc));
    var archive = Path.Combine(this._dir, "t.tar.gz");
    ArchiveOperations.Create(archive, [
      new ArchiveInput("", "sub/"),
      new ArchiveInput(Path.Combine(src, "a.txt"), "a.txt"),
      new ArchiveInput(Path.Combine(src, "sub", "a.txt"), "sub/a.txt"),
      new ArchiveInput(Path.Combine(src, "sub", "b.txt"), "sub/b.txt"),
    ], new CompressionOptions());
    return archive;
  }

  private static string[] Names(string archive)
    => ArchiveOperations.List(archive, null).Select(e => Normalize(e.Name)).Order(StringComparer.Ordinal).ToArray();

  [Test]
  public void Remove_RootFile_LeavesSameNamedFileInSubfolder() {
    var archive = this.MakeTarGz();
    ArchiveOperations.Remove(archive, ["a.txt"]);
    Assert.That(Names(archive), Is.EqualTo(new[] { "sub", "sub/a.txt", "sub/b.txt" }));
  }

  [Test]
  public void Remove_Folder_RemovesFolderEntryToo() {
    var archive = this.MakeTarGz();
    ArchiveOperations.Remove(archive, ["sub/"]);
    Assert.That(Names(archive), Is.EqualTo(new[] { "a.txt" }));
  }

  [Test]
  public void Remove_IsCaseSensitive() {
    if (OperatingSystem.IsWindows()) Assert.Inconclusive("the fixture stages a.txt and A.TXT through the volume, and NTFS keeps them apart only with per-directory case sensitivity on (docs/WINDOWS.md)");
    var archive = this.MakeTarGz();
    ArchiveOperations.Remove(archive, ["A.TXT"]);
    Assert.That(Names(archive), Does.Contain("a.txt"));
  }

  [Test]
  public void Add_EmptyFolder_SurvivesRebuild() {
    var archive = this.MakeTarGz();
    ArchiveOperations.Add(archive, [new ArchiveInput("", "empty/")]);
    Assert.That(Names(archive), Does.Contain("empty"));
  }

  [Test]
  public void Rebuild_KeepsEntryTimes() {
    var archive = this.MakeTarGz();
    ArchiveOperations.Remove(archive, ["sub/b.txt"]);
    var entry = ArchiveOperations.List(archive, null).Single(e => Normalize(e.Name) == "sub/a.txt");
    Assert.That(entry.LastModified?.ToUniversalTime(), Is.EqualTo(new DateTime(2024, 1, 2, 3, 4, 5, DateTimeKind.Utc)));
  }

  [Test]
  public void Zip_AddFolder_ToExistingArchive_KeepsFolder() {
    var file = Path.Combine(this._dir, "f.txt");
    File.WriteAllText(file, "x");
    var archive = Path.Combine(this._dir, "t.zip");
    ArchiveOperations.Create(archive, [new ArchiveInput(file, "f.txt")], new CompressionOptions());
    ArchiveOperations.Add(archive, [new ArchiveInput("", "folder/")]);
    Assert.That(Names(archive), Is.EqualTo(new[] { "f.txt", "folder" }));
  }

  [Test]
  public void Remove_TraversalEntryName_DeletesNothingOutsideTheArchive() {
    var archive = this.MakeTarGz();
    var sentinel = Path.Combine(Path.GetTempPath(), $"fb_semantics_sentinel_{Guid.NewGuid():N}.txt");
    File.WriteAllText(sentinel, "keep");
    try {
      ArchiveOperations.Remove(archive, [$"../{Path.GetFileName(sentinel)}"]);
      Assert.That(Names(archive), Is.EqualTo(new[] { "a.txt", "sub", "sub/a.txt", "sub/b.txt" }));
      Assert.That(File.Exists(sentinel), Is.True);
    } finally {
      try { File.Delete(sentinel); } catch (IOException) { }
    }
  }
}
