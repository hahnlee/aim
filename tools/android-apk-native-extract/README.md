# Android APK native-library extraction boundary

This standalone gate turns one APK's `lib/arm64-v8a/*.so` entries into the
already-supported immutable sibling-directory input of
`aim_elf_discover_sibling_graph`. It does not implement or alter ART's
class-loader namespace/cache policy.

The boundary is byte-oriented. ZIP filenames and payloads are not decoded as
text while parsing. Native output leaves are preserved as Unix filename bytes;
the downstream closed ELF namespace still deliberately requires UTF-8
`DT_SONAME`/`DT_NEEDED` keys. Both ZIP method 0 (stored) and method 8 (raw
deflate) are accepted for selected native libraries. Other compression methods,
encryption, masked headers, multi-disk ZIP, and ZIP64 fail closed.

The parser locates a terminal EOCD, verifies exact central-directory bounds and
entry count, then cross-checks every central record with its local header and
rejects overlapping local data. It rejects absolute/backslash/NUL/dot/parent
paths, duplicate raw names, symlinks, nested `arm64-v8a` entries, CRC or size
mismatches, and native graphs above 64 files, 64 MiB per file, or 256 MiB total.
The whole APK is capped at 512 MiB and 4096 entries.
The executable opens the APK first, rejects descriptor metadata outside the
size cap before allocation, then reads at most cap-plus-one bytes from that same
descriptor and requires the observed length to match. A pathname replacement,
growth, or truncation cannot silently change the resource being parsed.

Extraction creates a fresh `0700` staging directory in the destination parent,
writes each file with create-new semantics, verifies decompressed size and CRC,
fsyncs it, changes files to `0400`, changes the directory to `0500`, and publishes
the complete directory with Darwin `renamex_np(RENAME_EXCL)`. Any
pre-publication failure removes the staging tree. Existing destinations are
never replaced, including across the final existence-check race. This is the
Darwin replacement for a Linux `memfd`: the loader receives an owned, private,
read-only directory and opens exact siblings through its `O_NOFOLLOW` dirfd
broker.

The extracted directory is not a search path. Recursive dependencies come only
from exact sibling `DT_NEEDED` components; APK paths, ELF RPATH/RUNPATH, dyld,
host paths, and alternate directories are never consulted. The caller must keep
the directory private and retain it through graph unload, then make it writable
and remove it as one owned tree.

`audit.sh` pins its control-flow comparison to Android 16
`android-16.0.0_r1`, builds a real NDK r28c AArch64 root/child/grandchild graph,
puts the graph in a deflated APK alongside arbitrary non-text bytes, extracts it,
and sends the read-only result through the existing recursive graph discovery,
mapping, relocation, constructor, `JNI_OnLoad`, and unload path. It also runs
parser/rollback negative tests.

The runtime integration is exercised by `art-bootstrap probe-runtime-elf-jni`.
That command creates a private temporary APK containing both three-level NDK
graphs (generic and registered JNI), runs this extractor, compares the SHA-256
of all six extracted DSOs with their NDK-built inputs, and retains the one owned
directory until host shutdown. Both ART fixture paths therefore use the same
process-wide filesystem authority. The generic extracted root goes through the
real `JavaVMExt::LoadNativeLibrary` path; the exact JNI root later exercises
partial rollback and registered-JNI loading from the same sealed directory. The
runtime verifies the `0400` root and `0500` non-symlink parent before
NativeBridge loading and emits a separate APK/root SHA-256 result only after
`DestroyJavaVM` has returned with zero live JNI trampolines. The libc++, TLS,
and registered-JNI behaviors remain separate regression cases.

Run:

```sh
./tools/android-apk-native-extract/audit.sh
```
