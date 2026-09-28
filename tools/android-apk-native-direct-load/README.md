# Android APK direct native-library boundary

This new-only gate models `android:extractNativeLibs="false"` without adding an
APK pathname parser to ART or the Darwin ELF loader. It opens one non-writable
regular APK with `O_RDONLY | O_NOFOLLOW`, takes a shared advisory lock, maps it
`MAP_PRIVATE | PROT_READ`, and constructs the existing
`AimElfGraphSource` array directly from validated ZIP data slices. It
creates no extracted DSO and copies no DSO payload before graph loading.

The source lock pins Android 16 `android-16.0.0_r1`: ART
`libnativeloader` loads with `RTLD_NOW` inside its selected namespace, while
`zipalign` only page-aligns uncompressed entries whose extension is `.so`.
Android 16 accepts `-P` page sizes 4, 16, and 64 KiB. This Darwin boundary
selects 16 KiB, matching the current host/Android ELF page contract.

Only direct `lib/arm64-v8a/*.so` entries using ZIP method 0 are eligible. The
parser verifies a terminal EOCD, exact central bounds/count, every central/local
name and metadata pair, non-overlapping data ranges, CRC32, 16 KiB local data
offsets, regular-file identity, raw-name uniqueness, and the same 64-file,
64-MiB-per-file, 256-MiB-graph caps as sibling discovery. Multi-disk, ZIP64,
encryption, masked headers, data descriptors, symlinks, path traversal, nested
ABI entries, deflated DSOs, and unknown recursive `DT_NEEDED` siblings fail
closed. RPATH, RUNPATH, dyld, and host-path fallback are never consulted.

The fd's device, inode, size, mode, mtime, and ctime are captured before mmap
and checked immediately before graph loading, after loading, and after unload.
The APK must have no write permission bits. This detects replacement,
truncation, chmod, and ordinary concurrent writes; the installed-APK owner must
still enforce that writers cannot ignore the advisory lock. The race gate
deliberately mutates an unrelated mapped asset after validation and proves the
pre-load identity check rejects it before executing the ELF graph.

`audit.sh` builds a real NDK r28c AArch64 root/child/grandchild graph, places it
in a hand-built 16-KiB-aligned STORED APK, loads it from mmap slices through the
existing recursive ELF graph API, checks dependency-first constructors and
`JNI_OnLoad`, then unloads it. It also covers every malformed policy case under
ASan, UBSan, and TSan.

Run:

```sh
./tools/android-apk-native-direct-load/audit.sh
```
