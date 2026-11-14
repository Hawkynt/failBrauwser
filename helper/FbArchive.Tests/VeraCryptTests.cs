using System.Security.Cryptography;
using Compression.Lib;
using Compression.Registry;

namespace FbArchive.Tests;

/// <summary>
/// The M13 contract: TrueCrypt/VeraCrypt container files open as read-only folders behind a
/// password. The five fixtures were made by the real VeraCrypt program (password "test", PIM 0),
/// so they are external ground truth: their inner volume images carry digests recorded when the
/// containers were created, and every wrong-password answer is the same password failure — a
/// VeraCrypt header holds no plaintext marker, so "wrong password" and "not a volume" cannot be
/// told apart and the honest answer is the one the password dialog re-asks on.
/// These tests decrypt for real: an open tries the header's derivations side by side and stops the
/// ones still hashing as soon as one of them opens, so a right password costs about its own
/// derivation — seconds for the SHA fixtures, tens of seconds for whirlpool, blake2s or streebog,
/// whose hashes run as managed code here — while a wrong password or noise pays the whole trial,
/// which no search can shorten. That remaining time is the volume's arithmetic, not overhead.
/// </summary>
[TestFixture]
public class VeraCryptTests {

  private string _root = "";

  [SetUp]
  public void MakeRoot() => _root = Directory.CreateTempSubdirectory("fb-vc-").FullName;

  [TearDown]
  public void DropRoot() => Directory.Delete(_root, true);

  private static string Fixture(string name) => Path.Combine(AppContext.BaseDirectory, "veracrypt", name);

  private static List<string> Names(string archive, string? password) =>
    ArchiveOperations.List(archive, password).Where(e => !e.IsDirectory).Select(e => Commands.Normalize(e.Name)).ToList();

  // ── discovery ──

  [Test]
  public void AContainerFileIsDetectedByItsVolumeExtension() {
    Assert.That(FormatDetector.Detect(Fixture("test.sha512.hc")), Is.EqualTo(FormatDetector.Format.VeraCrypt));
  }

  [Test]
  public void TheFormatIsRegisteredForListingNotForWriting() {
    var d = FormatRegistry.GetById("VeraCrypt");
    Assert.That(d, Is.Not.Null);
    Assert.That(d!.Extensions, Is.EquivalentTo(new[] { ".hc", ".tc" }));
    Assert.That(d.Capabilities.HasFlag(Compression.Registry.FormatCapabilities.SupportsPassword), Is.True);
    Assert.That(d.Capabilities.HasFlag(Compression.Registry.FormatCapabilities.CanList), Is.True);
    Assert.That(d.Capabilities.HasFlag(Compression.Registry.FormatCapabilities.CanModify), Is.False);
    Assert.That(Commands.CanCreate(d), Is.False, "new encrypted volumes are not offered (M13)");
  }

  // ── opening with the password ──

  [Test]
  public void ThePasswordOpensTheContainerIntoItsInnerFiles() {
    Assert.That(Names(Fixture("test.sha512.hc"), "test"), Is.EquivalentTo(new[] { "metadata.ini", "volume.img" }));
    var volume = ArchiveOperations.List(Fixture("test.sha512.hc"), "test").Single(e => Commands.Normalize(e.Name) == "volume.img");
    Assert.That(volume.OriginalSize, Is.EqualTo(262144));
  }

  [Test]
  public void ExtractingGivesTheInnerVolumeAndItsMetadata() {
    var dest = Path.Combine(_root, "out");
    Commands.Extract(Fixture("test.sha512.hc"), dest, null, "test");
    var files = new DirectoryInfo(dest).GetFiles("*", SearchOption.AllDirectories);
    Assert.That(files.Select(f => Commands.Normalize(Path.GetRelativePath(dest, f.FullName))), Is.EquivalentTo(new[] { "metadata.ini", "volume.img" }));
    var inner = files.Single(f => f.Name == "volume.img");
    Assert.That(inner.Length, Is.EqualTo(262144));
    Assert.That(Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(inner.FullName))),
      Is.EqualTo("eef460d147e613d70df39a8e6e83b624fc1d258d8ae35875caf6579a05cba95b"));
  }

  /// <summary>The five real containers decrypt to the data-area digests recorded when they were made.</summary>
  [TestCase("test.sha512.hc", "eef460d147e613d70df39a8e6e83b624fc1d258d8ae35875caf6579a05cba95b")]
  [TestCase("test.sha256.hc", "2b15dbe4544a853ca15311dd1dc83162c6cc06c6c470d98ca427b26093a9fb41")]
  [TestCase("test.blake2s.hc", "21cc4f2f8db13fadca82bebb2ad574184dcbb441d447a5dee7363ceac729e0c0")]
  [TestCase("test.whirlpool.hc", "cb194e676678b7c40eead878df102eee0c93d453ca9d0a9e4eab7a3847edd1e0")]
  [TestCase("test.streebog.hc", "4c890502857a6360ed059c4d1dc736179fe29bffc9af72bfbff63bb691d34d82")]
  public void EachRealFixtureMatchesItsRecordedDigest(string fixture, string expectedDigest) {
    var dest = Path.Combine(_root, Path.GetFileNameWithoutExtension(fixture));
    Commands.Extract(Fixture(fixture), dest, null, "test");
    var inner = new DirectoryInfo(dest).GetFiles("volume.img", SearchOption.AllDirectories).Single();
    Assert.That(Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(inner.FullName))), Is.EqualTo(expectedDigest), fixture);
  }

  // ── the password failure the dialog re-asks on ──
  //
  // A container no password opens still lists its unchanged shell — the one volume.img whose method
  // says "encrypted" — and that is how the program knows a password is asked for or was the wrong
  // one. The password failure itself is the answer to the work behind it, extract and mounting:
  // UnauthorizedAccessException naming the password, which is what makes the dialog re-ask.

  [Test]
  public void AWrongPasswordAnswersWithAPasswordFailure() {
    var locked = ArchiveOperations.List(Fixture("test.sha512.hc"), "battery-staple");
    Assert.That(locked.Select(e => Commands.Normalize(e.Name)), Is.EqualTo(new[] { "volume.img" }), "a shell stays listed while the dialog re-asks");
    Assert.That(locked.Single().Method, Is.EqualTo("encrypted"), "and says it is not open");
    var error = Assert.Catch(() => Commands.Extract(Fixture("test.sha512.hc"), Path.Combine(_root, "wrong"), null, "battery-staple"));
    Assert.That(error, Is.InstanceOf<UnauthorizedAccessException>());
    Assert.That(error!.Message, Does.Contain("password"));
  }

  [Test]
  public void WithoutAPasswordTheAnswerSaysAPasswordIsNeeded() {
    var error = Assert.Catch(() => Commands.Extract(Fixture("test.sha512.hc"), Path.Combine(_root, "none"), null, null));
    Assert.That(error, Is.InstanceOf<UnauthorizedAccessException>());
    Assert.That(error!.Message, Does.Contain("password"));
  }

  [Test]
  public void AFileThatIsNotAVolumeAnswersTheSamePasswordFailure() {
    var noise = Path.Combine(_root, "noise.hc");
    File.WriteAllBytes(noise, RandomNumberGenerator.GetBytes(4096));
    // No plaintext marker exists in an encrypted header: the honest answer stays the password one.
    var error = Assert.Catch(() => Commands.Extract(noise, Path.Combine(_root, "noise-out"), null, "test"));
    Assert.That(error, Is.InstanceOf<UnauthorizedAccessException>());
    Assert.That(error!.Message, Does.Contain("password"));
  }

  // ── read-only (M13) ──

  [Test]
  public void EncryptedVolumesAreNeverWritable() {
    var format = FormatDetector.Format.VeraCrypt;
    Assert.That(Commands.IsWritable(format, Fixture("test.sha512.hc")), Is.False);
    Assert.That(Commands.ReadOnlyReason(format, Fixture("test.sha512.hc")), Does.Contain("read-only"));
  }

  [Test]
  public void EditingAContainerIsRefusedWithoutEvenTryingThePassword() {
    var error = Assert.Catch(() => Commands.Remove(Fixture("test.sha512.hc"), ["volume.img"], "test"));
    Assert.That(error, Is.InstanceOf<NotSupportedException>());
    Assert.That(error!.Message, Does.Contain("read-only"));
  }

  [Test]
  public void ANewEncryptedVolumeIsRefused() {
    var target = Path.Combine(_root, "fresh.hc");
    var source = Path.Combine(_root, "a.txt");
    File.WriteAllText(source, "alpha");
    var error = Assert.Catch(() => Commands.Add(target, [new(source, "a.txt")], "test", "VeraCrypt"));
    Assert.That(error, Is.InstanceOf<NotSupportedException>());
    Assert.That(File.Exists(target), Is.False);
  }
}
