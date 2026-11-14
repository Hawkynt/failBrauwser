using System.Security.Cryptography;
using Compression.Lib;

namespace FbArchive.Tests;

/// <summary>
/// The M14 contract: Open as &lt;kind&gt;. A VeraCrypt header is random-looking bytes, so a
/// container renamed to a lying name (or given none) cannot be found by detection at all —
/// the user's choice of kind is what opens it. A wrong choice says so and opens nothing.
/// </summary>
[TestFixture]
public class OpenAsTests {

  private string _root = "";

  [SetUp]
  public void MakeRoot() => _root = Directory.CreateTempSubdirectory("fb-openas-").FullName;

  [TearDown]
  public void DropRoot() => Directory.Delete(_root, true);

  private string LyingCopy() {
    // The blake2s container under a name that claims something else entirely.
    var bin = Path.Combine(_root, "disk.bin");
    File.Copy(Path.Combine(AppContext.BaseDirectory, "veracrypt", "test.blake2s.hc"), bin);
    return bin;
  }

  [Test]
  public void AContainerUnderALyingNameIsNotFoundByDetection() {
    // Characterizes the gap the choice answers: .bin is claimed by an image format long before
    // any volume idea, and content detection cannot see an encrypted header at all.
    var lying = LyingCopy();
    Assert.That(FormatDetector.Detect(lying), Is.Not.EqualTo(FormatDetector.Format.VeraCrypt), "detection alone cannot answer a VeraCrypt container");
    // Detection does answer it — just with the wrong kind (the .bin claims an image format), so an
    // unforced read cannot reach the volume. Whatever it produces, the real volume digest never shows.
    var outDir = Path.Combine(_root, "unforced");
    try { Commands.Extract(lying, outDir, null, "test"); } catch (Exception) { /* the wrong kind throws or yields junk: both prove the gap */ }
    if (!Directory.Exists(outDir)) return;
    var inner = new DirectoryInfo(outDir).GetFiles("volume.img", SearchOption.AllDirectories);
    foreach (var f in inner)
      Assert.That(Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(f.FullName))),
        Is.Not.EqualTo("21cc4f2f8db13fadca82bebb2ad574184dcbb441d447a5dee7363ceac729e0c0"),
        "whatever detection made of it, it was not the volume");
  }

  [Test]
  public void AChosenKindOpensAContainerItsNameCannotTell() {
    var dest = Path.Combine(_root, "out");
    Commands.Extract(LyingCopy(), dest, null, "test", "VeraCrypt");
    var inner = new DirectoryInfo(dest).GetFiles("volume.img", SearchOption.AllDirectories).Single();
    Assert.That(Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(inner.FullName))),
      Is.EqualTo("21cc4f2f8db13fadca82bebb2ad574184dcbb441d447a5dee7363ceac729e0c0"), "the forced kind reads the real volume");
  }

  /// <summary>
  /// A container another program holds open is a sharing state, not a password state and not a
  /// resource id: this is what a mounted VeraCrypt volume answers, since VeraCrypt keeps its
  /// container closed to every other reader at any sharing.
  /// </summary>
  [Test]
  public void AFileAnotherProgramHoldsOpenAnswersInUseAndNamesTheFile() {
    var held = Path.Combine(_root, "held.hc");
    File.Copy(Path.Combine(AppContext.BaseDirectory, "veracrypt", "test.sha256.hc"), held);
    using var blocker = new FileStream(held, FileMode.Open, FileAccess.ReadWrite, FileShare.None);
    var error = (Exception?) Assert.Catch(() => Commands.Extract(held, Path.Combine(_root, "held-out"), null, "test", "VeraCrypt"));
    Assert.That(error, Is.Not.Null, "a held container must answer, not half-read or hang");
    Assert.That(ErrorCodes.Code(error!), Is.EqualTo("inuse"), "the holder's handle is the answer, not a password or a mystery");
    Assert.That(ErrorCodes.Message(error!), Does.Contain("in use").And.Contain("held.hc"), "what the user reads names the file and the state");
  }

  [Test]
  public void AWrongChosenKindSaysSoAndOpensNothing() {
    var dest = Path.Combine(_root, "wrong-out");
    var error = Assert.Catch(() => Commands.Extract(LyingCopy(), dest, null, "test", "BitLocker"));
    Assert.That(error, Is.Not.Null, "the wrong kind must answer, not open an empty folder");
    Assert.That(new DirectoryInfo(dest).GetFiles("*", SearchOption.AllDirectories), Is.Empty, "and it opens nothing");
  }

  [Test]
  public void AnUnknownChosenKindIsRefusedByName() {
    var error = Assert.Catch(() => Commands.Extract(LyingCopy(), Path.Combine(_root, "nope"), null, "test", "NoSuchKind"));
    Assert.That(error, Is.InstanceOf<NotSupportedException>());
    Assert.That(error!.Message, Does.Contain("NoSuchKind"));
  }
}
