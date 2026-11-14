# CompressionWorkbench (vendored)

The archive support of failBrauwser is the work of **[@Hawkynt](https://github.com/Hawkynt)**:
[CompressionWorkbench](https://github.com/Hawkynt/CompressionWorkbench), a library that reads
and writes hundreds of archive, compression and file system formats. failBrauwser only adds a
small helper process (`helper/FbArchive`) that speaks to it.

This folder holds the parts of CompressionWorkbench that `Compression.Lib` needs, so
failBrauwser builds on its own:

| Project | What it is |
|---|---|
| `Compression.Lib` | format detection and archive operations (list, extract, add, remove, create) |
| `Compression.Core` | compression primitives and shared containers (ZIP, …) |
| `Compression.Registry`, `Compression.Registry.Generator` | format contracts and their source-generated registration |
| `Hawkynt.FileFormats.Archives`, `.FileSystems`, `.Audio` | the formats themselves |
| `FileFormats/FileFormat.Ani`, `FileFormats/FileFormat.Ico` | Windows cursors and icons (read through `Hawkynt.FileFormats.Images`) |
| `Hawkynt.Algorithms.Checksums`, `.Hashing` | checksums and hashes |

Upstream: https://github.com/Hawkynt/CompressionWorkbench, commit `ab61c59f` of 2026-11-13
("Integrate verified feature pools, archive codecs, ISO and writable mounts": StuffIt classic
encryption and resource forks, UHARC 0.4, ISO Rock Ridge writing, ZFS pool edits), applied on
top of `b9c490df` ("Integrate multi-device ZFS edits and StuffIt 5 encryption"). Until then failBrauwser carried
a copy of upstream `17200a6d` (2026-09-28) with the changes listed below; that copy was merged
into the new one, and where both had changed a file, upstream's version was taken.

License: LGPL-3.0-or-later (`LICENSE` here); third-party notices in
`THIRD-PARTY-NOTICES.md`. The NuGet package `Hawkynt.FileFormats.Images` (also by Hawkynt)
is fetched at build time.

## Changes made for failBrauwser

Still local (candidates for upstream pull requests):

- (fix) removing entries through a rebuild matches names exactly in case
  (`RebuildVerb.DeleteNamedEntries`): on Linux `A.txt` and `a.txt` are two entries, and
  removing one removed both
- (fix) NTFS, FAT and ext remove each entry once when a folder and entries inside it are both
  named; the second removal failed with "not found"
- `Compression.Registry/RebuildStaging.cs`, failBrauwser's former rebuild staging, kept for the
  helper's tests (`RebuildStaging.Normalize`)
- (perf) VeraCrypt/TrueCrypt unlocking (`TcKdf.cs`, `TcVolume.cs`): the header-key derivations
  of all candidate headers run side by side and stop once one opens, PBKDF2 keeps its pads and
  hashes spans instead of allocating per round; a wrong passphrase is refused in seconds instead
  of minutes. Upstream's order (standard header, then hidden) is kept: standard candidates come
  first and the earliest header wins. Pinned by `helper/FbArchive.Tests/VeraCryptKdfTests.cs`.
- (build) `Compression.Registry.Generator.csproj` pins Roslyn to 5.0.* and the analyzers to
  3.11.0: the 5.3 analyzers do not load on the .NET 10.0.10x SDK compiler (CS9057), which the
  Windows build uses

Earlier changes, all taken over upstream by 2026-11-13 (in its own words and code):

- `39965dfa` (test) rebuild edit semantics: exact names, folders, case, times
- `950b150e` (fix) tar keeps times and executable bits, lists times as UTC
- `478d366d` (fix) zip keeps entry times and adds folder entries to existing archives
- `0f702049` (feat) inputs carry source timestamp and mode; helper to restore entry times on extract
- `9350e56f` (fix) rebuild-based add and remove no longer match entries by leaf name or drop empty folders
- `7ad59278` (fix) shared rebuild staging: exact entry names, folders removed with their contents
- (feat) `ArchiveInputInfo.ReadObserver` reports bytes read, for progress bars
- (fix) ISO reader prefers Rock Ridge names and reads PX, TF, NM continuations and CE areas
- (feat) ISO writer: Rock Ridge, folders, file metadata, unique ISO 9660 identifiers
- (feat) `IsoBoot`: read El Torito entries, detect hybrid images; the writer writes boot
  catalogs with BIOS and UEFI entries and patches the isolinux boot info table
- (fix) format ids are matched regardless of case (xDisk, xMash were never recognised)
- (fix) new ISO images keep folders, with Rock Ridge; CDI, MDF, NRG and BIN images reuse it
- (fix) ext2/3/4: new images keep folders, empty ones too; rebuilds keep and add folders
- (fix) NTFS: new images keep folders; folders can be added and removed
- (fix) XAR: table-of-contents checksum, folders and modes; edits keep the tree
- (fix) FAT: empty folders; VHD, VHDX, VMDK, VDI, QCOW2 keep long names and folders
- (bug) BBC DFS removed files of the same name in another directory; extraction names
- (bug) BKF edits went to the wrong folder and removed same-named files everywhere
- (bug) the shared rebuild path dropped entries by leaf name; it keeps folders now
- (bug) MFS-1 adds cut the other names down to their extension
- (fix) behind a generic suffix (.bin, .img, .dat, …) a clear signature decides the format

Later changes are in failBrauwser's git history (`git log -- vendor/compressionworkbench`);
their tests are in `helper/FbArchive.Tests`. They are candidates for upstream pull requests.
