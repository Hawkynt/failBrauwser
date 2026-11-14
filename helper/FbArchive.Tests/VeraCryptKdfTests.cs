using FileFormat.VeraCrypt;
using Hawkynt.Algorithms.Hashing;

namespace FbArchive.Tests;

/// <summary>
/// Unlocking a container leans on PBKDF2 over hashes the framework does not implement: twelve
/// derivations per header copy, each one deriving a 192-byte key, so this loop is the wall a
/// password waits behind. Making it lean (reused buffers, the key's blocks derived in parallel)
/// has to stay bit-identical, so every PRF without a framework implementation is checked against
/// an outside one — OpenSSL's PBKDF2 as Python's hashlib exposes it for RIPEMD-160 and BLAKE2s,
/// and a textbook RFC 8018 loop written here for Whirlpool and Streebog, which no foreign tool
/// computes. The real containers in <see cref="VeraCryptTests"/> are the end-to-end proof.
/// </summary>
[TestFixture]
public class VeraCryptKdfTests {

  private static ReadOnlySpan<byte> Password => "password"u8;
  private static ReadOnlySpan<byte> Salt => "salt"u8;

  // ── against another implementation entirely ──
  //
  // hashlib.pbkdf2_hmac(algorithm, b"password", b"salt", rounds, 192).hex()

  [TestCase("ripemd160", 1, "b725258b125e0bacb0e2307e34feb16a4d0d6aed6cb4b0eee458fc18290204289e55d962783bf52237d264cbbab25f18d89d8c798f90f558ea7b45bdf3d083340c18b9d23ba842183c5364d18bc0ffde5a8a408dd7ef02dde561a08d21c6d2325a69869b13877a9d0b2f8811cf2c2f86b4034f1f7d061337a167a26a3b59460a327e0cc9b8b6579df3096c7da284387172a8247b425a424fe0d1238a959419b0f23379e9a25a13846f024cce77742a0883da1b7254a467a0712cd7c1039e5f19")]
  [TestCase("ripemd160", 2, "768dcc27b7bfdef794a1ff9d935090fcf598555e66913180b9ce363c615e9ed953b95fd07169be535e38afbea29c030e06d14f40745b1513b7ccdf0e76229e503ab2bf37278b32ac5238620daf43a5f9274590cb01bbe74a124e9bd9028cac33d13a97ad799e69921eb0b64a74392db0d8d63aa1f8366798b8532ce51b33433bd682abc9f8dce47fa7f409cd137250eb6a6ddefc675bb99bccd8799c89701cf8be9fd4b9a4a73ba7b8e9b50acc42d916f80813c4dd78868a9fc47ae4a273a7e2")]
  [TestCase("ripemd160", 4096, "99a40d3fe4ee95869791d9faa24864562782762171480b620ca8bed3dafbbcac85d6a7c5b52f2f99882786b0805295ae340cba8e0701914243f3052e147f7fd9519387ed56d1be9279f2cb3cff31cb8f8c70630df0bed267556ce4c89b90f282825911e04d2da3a658bfc8e8556ad9d9d09c9938a4b28c3425cb292d50efe1744a57fcca785c4fb348575ed3e0313e09b1569b9c32f4560013604b653190a8f8a9d55a683106e06a63c4c36086c38a9a48ce3a6d47987f7a68c84190967a114d")]
  [TestCase("ripemd160", 10_000, "be977b986ed2fa16c9eb65d0bf8655e14d2e9233c941cb2bff06c3e55adf3bab8f9b5791075adffcebb9d7e9f5e17928d1452f40954d8647b50f15cca210693bf3fa7704bd71ecf296c90e68f8e7512ee847506b9328fa6cc533bb06b46a3fc6e4bab8234d60702da9b3054cfd0509596879d60827347461aba0516355021d4ea60328bee3e9d9b486fb8de2a463c8f9aad43a981efcccd0516a14f4dd13d0e256bc6e7aafa9c96f93f32c5259ba659ce2ff604c221ab99177c22bcd61c2ae78")]
  [TestCase("blake2s-256", 1, "ed939d6f351dddb69f591aa693d75eccaab7c8f587384d8ed882d42fe80764744956babeb2be9f64c09b9b4e6833d32ab8c791cb8b47196d93e286e9d3ad3bfce260699a52676ab95d075b933e8688060e29840ae853e7550b105b603c560133b14eaa12c46af77a944a916b5f6a2df720cc36c26cf4f70b0c96d4acfa3d85015b98fe8530d0e8f02db27b822036fe97cde30fd5ae8319554f5b77e8ff62bef5caa4ac39fa02623fc8535d4a9b51965e37b459aff3df29c3465f49db7480b2a0")]
  [TestCase("blake2s-256", 2, "7b88e65e6e95a118bc995f681a391cbd7b46e0cf9750a81100f613add62d84be7f88c3e2900b32fd3ed5e97b3c96acc8ba1bfc58d78e92c0da2426e9b3bbc14798da318dee332271a590f0830ef0fd1d15faceb22daf91902e3b4b8464e21556cd5ff3c74419c8b8890253737779cc61cd6ce6ddf2589002c9a0d6de796d2238e4f783708372ce67f0a3508a375f171a55d90db5f7ede2a5a11449124ae8ef7f6ca8c9982fd05410119e2d8da7eba330e64fc782f5c71e83a33cdadb6dcb30a2")]
  [TestCase("blake2s-256", 4096, "072b63e2cfe4d20cd2086a6be6ec8e1fd1bf2b797fa272a749a761faad66beb61c8071a4aae2fe3e051b59ed2d0a1dc4305e9e926a1fb56dddb06ef635837a6320bd46a1ac73d439a207128d12b51df567e2bc8b21e2fffd08ee7e69bbdb543bdd7548d1088e36cdc01d876dfd65a33158e3282fad7d227ade94742bf2557aae6837414ab3b73388cddd1a3e67f823a62ec44f0580a91f4e5f11b8b8082697d3435a723c68e88eb91a21948fda176012f478f96ccccbfb144e790302593f4fb1")]
  [TestCase("blake2s-256", 10_000, "41ff26a690e2da3d70a96c2cee90a842c3bb548d79025bb88700655a80980cc87c2f67326786cac0ef917de3493c1f5db4b2769ca85d9285ed9b52def5480413aae1faf30e8e51403eba890ca0e540a2181f246da94697ed40613a9600b2c9dc6bbb7fcd9b475d17602c7b2d09397917467237acdf59b342a74fae881109085186f942d0a1957755dc8827bd93d01c15e278cafcca776a3be4089845de78ad8ee6e48ce3f98350ba51661ab374becdee4b8455310bbbefe9c1472f775c8d0447")]
  public void Pbkdf2MatchesAnotherImplementation(string prf, int rounds, string expectedHex) {
    var derived = Derive(prf, rounds);
    Assert.That(derived, Has.Length.EqualTo(TcKdf.HeaderKeyLength), $"{prf} × {rounds}");
    Assert.That(Convert.ToHexStringLower(derived), Is.EqualTo(expectedHex), $"{prf} × {rounds}");
  }

  // ── against a textbook loop for the hashes no foreign tool computes ──

  [TestCase("whirlpool", 1)]
  [TestCase("whirlpool", 3)]
  [TestCase("whirlpool", 10_000)]
  [TestCase("stribog512", 1)]
  [TestCase("stribog512", 3)]
  [TestCase("stribog512", 10_000)]
  public void Pbkdf2MatchesTheTextbookLoop(string prf, int rounds) {
    Assert.That(Derive(prf, rounds), Is.EqualTo(Textbook(prf, rounds)), $"{prf} × {rounds}");
  }

  // ── a derivation a search overtakes says so instead of answering with a part-derived key ──

  [TestCase("ripemd160", 655_331)]
  [TestCase("blake2s-256", 500_000)]
  [TestCase("whirlpool", 500_000)]
  [TestCase("stribog512", 500_000)]
  public void AStoppedDerivationAnswersWithTheStop(string prf, int rounds) {
    using var stopped = new CancellationTokenSource();
    stopped.Cancel();
    Assert.That(() => Derive(prf, rounds, stopped.Token), Throws.TypeOf<OperationCanceledException>(), $"{prf} × {rounds}");
  }

  private static byte[] Derive(string prf, int rounds, CancellationToken cancel = default) => prf switch {
    "ripemd160" => TcKdf.Pbkdf2(static d => Ripemd160.Compute(d), 64, Password, Salt, rounds, cancel),
    "blake2s-256" => TcKdf.Pbkdf2(static d => Blake2s.Compute(d), 64, Password, Salt, rounds, cancel),
    "whirlpool" => TcKdf.Pbkdf2(static d => Whirlpool.Compute(d), 128, Password, Salt, rounds, cancel),
    "stribog512" => TcKdf.Pbkdf2(static d => Streebog512.Compute(d), 128, Password, Salt, rounds, cancel),
    _ => throw new ArgumentOutOfRangeException(nameof(prf)),
  };

  /// <summary>RFC 8018 written for reading rather than for speed: every round re-keys and re-hashes the pad.</summary>
  private static byte[] Textbook(string prf, int rounds) {
    Func<ReadOnlySpan<byte>, byte[]> hash = prf switch {
      "whirlpool" => static d => Whirlpool.Compute(d),
      "stribog512" => static d => Streebog512.Compute(d),
      _ => throw new ArgumentOutOfRangeException(nameof(prf)),
    };
    const int blockSize = 128;
    var key = Password.Length > blockSize ? hash(Password) : Password.ToArray();
    var derived = new byte[TcKdf.HeaderKeyLength];
    for (int block = 1, done = 0; done < derived.Length; ++block) {
      byte[] message = [.. Salt.ToArray(), (byte)(block >> 24), (byte)(block >> 16), (byte)(block >> 8), (byte)block];
      var u = Hmac(hash, blockSize, key, message);
      var t = (byte[])u.Clone();
      for (var round = 1; round < rounds; ++round) {
        u = Hmac(hash, blockSize, key, u);
        for (var i = 0; i < t.Length; ++i) t[i] ^= u[i];
      }
      t.AsSpan(0, Math.Min(t.Length, derived.Length - done)).CopyTo(derived.AsSpan(done));
      done += t.Length;
    }
    return derived;
  }

  private static byte[] Hmac(Func<ReadOnlySpan<byte>, byte[]> hash, int blockSize, byte[] key, ReadOnlySpan<byte> message) {
    var keyed = key.Length > blockSize ? hash(key) : key;
    var ipad = new byte[blockSize];
    var opad = new byte[blockSize];
    for (var i = 0; i < blockSize; ++i) {
      var k = i < keyed.Length ? keyed[i] : (byte)0;
      ipad[i] = (byte)(k ^ 0x36);
      opad[i] = (byte)(k ^ 0x5c);
    }
    var inner = new byte[blockSize + message.Length];
    ipad.CopyTo(inner, 0);
    message.CopyTo(inner.AsSpan(blockSize));
    var digest = hash(inner);
    var outer = new byte[blockSize + digest.Length];
    opad.CopyTo(outer, 0);
    digest.CopyTo(outer, blockSize);
    return hash(outer);
  }
}
