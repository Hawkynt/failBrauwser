namespace FbArchive.Tests;

/// <summary>
/// Disk images (VHD, VHDX, VMDK, VDI, QCOW2) wrap a FAT volume, CD images (CDI, MDF, NRG, BIN)
/// an ISO 9660 track; failBrauwser changed the
/// vendored code so that long names, folders and empty folders survive, as in a plain FAT image.
/// </summary>
[TestFixture]
public class DiskImageTests {

  [TestCase("Fat", ".img")]
  [TestCase("Vhd", ".vhd")]
  [TestCase("Vhdx", ".vhdx")]
  [TestCase("Vmdk", ".vmdk")]
  [TestCase("Vdi", ".vdi")]
  [TestCase("Qcow2", ".qcow2")]
  [TestCase("Iso", ".iso")]
  public void KeepsLongNamesAndFolders(string format, string ext) {
    var root = Directory.CreateTempSubdirectory("fb-disk-");
    try {
      Directory.CreateDirectory(Path.Combine(root.FullName, "Pack", "Sub Folder", "empty"));
      File.WriteAllText(Path.Combine(root.FullName, "Pack", "Sub Folder", "A long Name.txt"), "deep");
      var img = Path.Combine(root.FullName, "disk" + ext);
      Commands.Add(img, [new(Path.Combine(root.FullName, "Pack"), "Pack")], null, format);
      var names = Compression.Lib.ArchiveOperations.List(img, null).Select(e => Commands.Normalize(e.Name)).ToList();
      Assert.That(names, Is.SupersetOf(new[] { "Pack", "Pack/Sub Folder", "Pack/Sub Folder/empty", "Pack/Sub Folder/A long Name.txt" }), string.Join(", ", names));
      var x = Path.Combine(root.FullName, "x");
      Commands.Extract(img, x, null, null);
      Assert.That(File.ReadAllText(Path.Combine(x, "Pack", "Sub Folder", "A long Name.txt")), Is.EqualTo("deep"));
      Assert.That(Directory.Exists(Path.Combine(x, "Pack", "Sub Folder", "empty")), Is.True);
    }
    finally {
      root.Delete(true);
    }
  }

  /// <summary>CD images keep folders; their readers show the ISO 9660 names (upper case).</summary>
  [TestCase("Cdi", ".cdi")]
  [TestCase("Mdf", ".mdf")]
  [TestCase("Nrg", ".nrg")]
  [TestCase("BinCue", ".bin")]
  public void CdImagesKeepFolders(string format, string ext) {
    var root = Directory.CreateTempSubdirectory("fb-cd-");
    try {
      Directory.CreateDirectory(Path.Combine(root.FullName, "Pack", "Sub", "empty"));
      File.WriteAllText(Path.Combine(root.FullName, "Pack", "Sub", "b.txt"), "deep");
      var img = Path.Combine(root.FullName, "disc" + ext);
      Commands.Add(img, [new(Path.Combine(root.FullName, "Pack"), "Pack")], null, format);
      var names = Compression.Lib.ArchiveOperations.List(img, null).Select(e => Commands.Normalize(e.Name).ToLowerInvariant()).ToList();
      Assert.That(names, Is.SupersetOf(new[] { "pack", "pack/sub", "pack/sub/empty", "pack/sub/b.txt" }), string.Join(", ", names));
      var x = Path.Combine(root.FullName, "x");
      Commands.Extract(img, x, null, null);
      var file = Directory.GetFiles(x, "*", SearchOption.AllDirectories).Single();
      Assert.That(File.ReadAllText(file), Is.EqualTo("deep"));
      Assert.That(Path.GetRelativePath(x, file).Replace('\\', '/').ToLowerInvariant(), Is.EqualTo("pack/sub/b.txt"));
    }
    finally {
      root.Delete(true);
    }
  }
}
