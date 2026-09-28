# Android 16 libandroidfw Darwin host foundation

This gate builds the Android 16.0.0_r1 host-static `libandroidfw` composition
from the checksum-locked `frameworks/base` revision
`99b01a65cc4c104933788b3143285ab6bae65827`.

The `libandroidfw` `Android.bp` selects 34 common sources on Darwin. Android's
`host_linux`-only `CursorWindow.cpp` and Android-only `BackupData.cpp`,
`BackupHelpers.cpp`, and `CursorWindow.cpp` are deliberately excluded. The
archive also contains the two `whole_static_libs` inputs required by the
module: `libandroidfw_pathutils` (`PathUtils.cpp`) and `libincfs-utils`
(`util/map_ptr.cpp`). Normal static/shared dependencies such as libpng, zlib,
libbase, libutils, liblog, and libziparchive remain external link providers.

The local source tree is intentionally gitless and sparse. The gate binds the
complete 34-file source manifest, `PathUtils.cpp`, and both incfs source/header
files by SHA-256. The sparse incfs materialization does not contain its
`Android.bp`; the lock records the upstream Android 16 manifest digest and the
single source selected by its host-supported `libincfs-utils` module. No
replacement source or per-symbol stub is accepted.

Run:

```sh
tools/build-android16-androidfw-foundation.sh
```

Successful output is `_build/androidfw-foundation/libandroidfw-darwin.a`, a
36-member arm64 archive. The gate force-loads it into a relocatable Mach-O
object, checks representative AssetManager/resource/zip/path/incfs symbols,
writes the transitive undefined-symbol manifest, and links a small executable
that exercises real `BigBuffer`, `PathUtils`, and `IncFsFileMap` definitions.

The generated undefined manifest is an acquisition/link-order input. It is not
filled with compatibility symbols. Final GraphicsJNI/HWUI integration must
satisfy it with module-complete dependency archives.
