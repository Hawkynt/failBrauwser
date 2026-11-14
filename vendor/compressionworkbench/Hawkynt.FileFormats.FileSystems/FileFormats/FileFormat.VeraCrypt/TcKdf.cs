using System.Buffers.Binary;
using System.Security.Cryptography;
using Compression.Core.Crypto;
using Hawkynt.Algorithms.Hashing;

namespace FileFormat.VeraCrypt;

/// <summary>
/// One header-key derivation TrueCrypt or VeraCrypt uses. The table, the iteration counts and the
/// PIM formulas are cryptsetup's (<c>lib/tcrypt/tcrypt.c</c>, LGPL-2.1-or-later).
/// </summary>
/// <param name="Prf">PRF name: <c>sha512</c>, <c>sha256</c>, <c>whirlpool</c>, <c>ripemd160</c>, <c>blake2s-256</c>, <c>stribog512</c>, <c>sha1</c>, or <c>argon2id</c>.</param>
/// <param name="Iterations">Iterations (Argon2: passes) without a PIM.</param>
/// <param name="VeraCrypt">A VeraCrypt derivation (magic <c>VERA</c>) rather than a TrueCrypt one.</param>
/// <param name="Legacy">Only TrueCrypt before 5.0 used it.</param>
/// <param name="PimConst">VeraCrypt: iterations = PimConst + PimMult × PIM.</param>
/// <param name="PimMult">See <paramref name="PimConst"/>.</param>
/// <param name="MemoryKiB">Argon2 memory without a PIM.</param>
public sealed record TcKdf(string Prf, int Iterations, bool VeraCrypt, bool Legacy, int PimConst, int PimMult, int MemoryKiB = 0) {
  public const int HeaderKeyLength = 192;

  /// <summary>Non-system volume derivations, cheapest-to-try first within each family.</summary>
  public static IReadOnlyList<TcKdf> All { get; } = [
    new("ripemd160", 2000, false, false, 0, 0),
    new("ripemd160", 1000, false, false, 0, 0),
    new("sha512", 1000, false, false, 0, 0),
    new("whirlpool", 1000, false, false, 0, 0),
    new("sha1", 2000, false, true, 0, 0),
    new("sha512", 500000, true, false, 15000, 1000),
    new("whirlpool", 500000, true, false, 15000, 1000),
    new("sha256", 500000, true, false, 15000, 1000),
    new("blake2s-256", 500000, true, false, 15000, 1000),
    new("ripemd160", 655331, true, false, 15000, 1000),
    new("stribog512", 500000, true, false, 15000, 1000),
    new("argon2id", 6, true, false, 0, 0, 425984),
  ];

  /// <summary>
  /// Derives the 192-byte header key from the (keyfile-mixed) password bytes. A search that already
  /// opened its header asks the derivations still running to stop: <paramref name="cancel"/> ends a
  /// PBKDF2 chain at its next checkpoint instead of finishing a key that nothing will read. Argon2id
  /// has no checkpoint because a search reaches it alone, after its parallel part found nothing.
  /// </summary>
  public byte[] Derive(ReadOnlySpan<byte> password, ReadOnlySpan<byte> salt, int pim = 0, CancellationToken cancel = default) {
    if (this.Prf == "argon2id") {
      var (passes, memory) = pim <= 0 ? (this.Iterations, this.MemoryKiB)
        : pim <= 31 ? ((pim - 1) / 3 + 3, 1024 * (64 + (pim - 1) * 32))
        : (pim - 18, 1024 * 1024);
      return Argon2.Hash(Argon2.Variant.Argon2id, password, salt, passes, memory, 1, HeaderKeyLength);
    }
    var iterations = pim > 0 && this.VeraCrypt ? this.PimConst + this.PimMult * pim : this.Iterations;
    return this.Prf switch {
      "sha512" => Rfc2898DeriveBytes.Pbkdf2(password, salt, iterations, HashAlgorithmName.SHA512, HeaderKeyLength),
      "sha256" => Rfc2898DeriveBytes.Pbkdf2(password, salt, iterations, HashAlgorithmName.SHA256, HeaderKeyLength),
      "sha1" => Rfc2898DeriveBytes.Pbkdf2(password, salt, iterations, HashAlgorithmName.SHA1, HeaderKeyLength),
      "ripemd160" => Pbkdf2(static d => Ripemd160.Compute(d), 64, password, salt, iterations, cancel),
      "whirlpool" => Pbkdf2(static d => Whirlpool.Compute(d), 64, password, salt, iterations, cancel),
      "blake2s-256" => Pbkdf2(static d => Blake2s.Compute(d), 64, password, salt, iterations, cancel),
      "stribog512" => Pbkdf2(static d => Streebog512.Compute(d), 64, password, salt, iterations, cancel),
      _ => throw new NotSupportedException($"TrueCrypt/VeraCrypt PRF '{this.Prf}' is not supported."),
    };
  }

  /// <summary>
  /// PBKDF2 (RFC 8018) over HMAC with a hash that has no framework implementation. A header key is
  /// 192 bytes, so these PRFs need several independent blocks (BLAKE2s six, RIPEMD-160 ten); above
  /// <see cref="BlockParallelFrom"/> an expensive derivation works its blocks side by side, which is
  /// the same arithmetic with less waiting. Keeping the round's buffers is what makes a 500 000-round
  /// chain cost its hash rather than the allocator.
  /// </summary>
  public static byte[] Pbkdf2(Func<ReadOnlySpan<byte>, byte[]> hash, int blockSize, ReadOnlySpan<byte> password, ReadOnlySpan<byte> salt, int iterations, CancellationToken cancel = default) {
    ArgumentNullException.ThrowIfNull(hash);
    ArgumentOutOfRangeException.ThrowIfNegativeOrZero(iterations);
    var key = password.Length > blockSize ? hash(password) : password.ToArray();
    var saltBytes = salt.ToArray(); // the blocks below capture it, and a span cannot be captured
    var digestLength = hash([]).Length;
    var output = new byte[HeaderKeyLength];
    var blocks = (HeaderKeyLength + digestLength - 1) / digestLength;
    void Block(int index) => DeriveBlock(hash, blockSize, digestLength, key, saltBytes, iterations, index + 1, output.AsSpan(index * digestLength, Math.Min(digestLength, HeaderKeyLength - index * digestLength)), cancel);
    try {
      if (blocks > 1 && iterations >= BlockParallelFrom) Parallel.For(0, blocks, new ParallelOptions { CancellationToken = cancel }, Block);
      else for (var index = 0; index < blocks; ++index) Block(index);
    } catch (AggregateException stops) when (stops.Flatten().InnerExceptions.All(static e => e is OperationCanceledException)) {
      throw stops.Flatten().InnerExceptions[0]; // the blocks were stopped by one ask, so the derivation answers with it
    } finally {
      CryptographicOperations.ZeroMemory(key);
      CryptographicOperations.ZeroMemory(saltBytes);
    }
    return output;
  }

  /// <summary>Below this iteration count a derivation's blocks are cheap enough to run in order.</summary>
  private const int BlockParallelFrom = 10_000;

  /// <summary>A stopped derivation is noticed this often during a long chain: often enough to give the
  /// answer a search already has, rare enough that reading the flag costs nothing measurable.</summary>
  private const int StopCheckEvery = 8192;

  /// <summary>One PBKDF2 block: the XOR chain <c>U₁ = PRF(P, S ‖ INT_be32(i))</c>, <c>Uₙ = PRF(P, Uₙ₋₁)</c>,
  /// <c>T = U₁ ⊕ … ⊕ U_c</c>, with its pads built once and its buffers reused across the rounds. A block
  /// that is asked to stop throws rather than writing a part-derived key, and its buffers are cleared
  /// either way.</summary>
  private static void DeriveBlock(Func<ReadOnlySpan<byte>, byte[]> hash, int blockSize, int digestLength, byte[] key, ReadOnlySpan<byte> salt, int iterations, int block, Span<byte> destination, CancellationToken cancel) {
    if (cancel.IsCancellationRequested) throw new OperationCanceledException(cancel);
    var inner = new byte[blockSize + Math.Max(salt.Length + 4, digestLength)];
    var outer = new byte[blockSize + digestLength];
    for (var i = 0; i < blockSize; ++i) {
      var k = i < key.Length ? key[i] : (byte)0;
      inner[i] = (byte)(k ^ 0x36);
      outer[i] = (byte)(k ^ 0x5c);
    }
    salt.CopyTo(inner.AsSpan(blockSize));
    BinaryPrimitives.WriteInt32BigEndian(inner.AsSpan(blockSize + salt.Length), block);
    var u = HmacRound(hash, blockSize, inner.AsSpan(0, blockSize + salt.Length + 4), outer);
    var t = (byte[])u.Clone();
    try {
      for (var round = 1; round < iterations; ++round) {
        if (round % StopCheckEvery == 0 && cancel.IsCancellationRequested) throw new OperationCanceledException(cancel);
        u.CopyTo(inner.AsSpan(blockSize));
        u = HmacRound(hash, blockSize, inner.AsSpan(0, blockSize + digestLength), outer);
        for (var k = 0; k < t.Length; ++k) t[k] ^= u[k];
      }
      t.AsSpan(0, destination.Length).CopyTo(destination);
    } finally {
      CryptographicOperations.ZeroMemory(t);
      CryptographicOperations.ZeroMemory(u);
      CryptographicOperations.ZeroMemory(inner);
      CryptographicOperations.ZeroMemory(outer);
    }
  }

  /// <summary>One HMAC round: <c>hash(opad ‖ hash(inner ‖ message))</c>, the inner digest written into the
  /// caller's outer pad, whose front half was keyed once for the whole block.</summary>
  private static byte[] HmacRound(Func<ReadOnlySpan<byte>, byte[]> hash, int blockSize, ReadOnlySpan<byte> innerMessage, byte[] outer) {
    var innerDigest = hash(innerMessage);
    innerDigest.CopyTo(outer.AsSpan(blockSize));
    CryptographicOperations.ZeroMemory(innerDigest);
    return hash(outer);
  }

  /// <summary>
  /// The password buffer the KDF sees: keyfiles are mixed into a 64-byte pool (128 bytes for a
  /// VeraCrypt password longer than 64), then the password is added byte by byte. With keyfiles
  /// the whole pool is used; without, only the password.
  /// </summary>
  public static byte[] MixPassword(ReadOnlySpan<byte> password, IReadOnlyList<byte[]> keyfiles, bool veraCrypt) {
    var poolLength = veraCrypt && password.Length > 64 ? 128 : 64;
    if (password.Length > poolLength)
      throw new ArgumentException($"TrueCrypt/VeraCrypt passwords are at most {poolLength} bytes.", nameof(password));
    var pool = new byte[poolLength];
    foreach (var file in keyfiles) {
      var crc = ~0u;
      var j = 0;
      foreach (var b in file.AsSpan(0, Math.Min(file.Length, 1 << 20))) {
        crc = Crc32Update(crc, b);
        pool[j++] += (byte)(crc >> 24);
        pool[j++] += (byte)(crc >> 16);
        pool[j++] += (byte)(crc >> 8);
        pool[j++] += (byte)crc;
        j %= poolLength;
      }
    }
    for (var i = 0; i < password.Length; ++i) pool[i] += password[i];
    return keyfiles.Count > 0 ? pool : pool.AsSpan(0, password.Length).ToArray();
  }

  /// <summary>One byte of the standard reflected CRC-32 without final inversion (zlib's running value).</summary>
  private static uint Crc32Update(uint crc, byte b) {
    crc ^= b;
    for (var k = 0; k < 8; ++k) crc = (crc & 1) != 0 ? (crc >> 1) ^ 0xEDB88320u : crc >> 1;
    return crc;
  }
}
