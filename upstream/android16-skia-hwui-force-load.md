# Android 16 Skia HWUI force-load closure

This gate produces a separate framework/Freetype Skia archive for whole-static
HWUI linking. It does not rewrite or prune `_build/skia-text/libskia.a`.

Skia's generated `gn/utils.gni` lists `SkCTFont.cpp` and
`SkCTFontCreateExactCopy.cpp` for every platform. Their implementations are
guarded by `SK_BUILD_FOR_MAC || SK_BUILD_FOR_IOS`; normal Android builds leave
those translation units empty through Skia's target-platform feature macro.
The previous Darwin GN build let `SkFeatures.h` select the implementations from
the compiler host even though `skia_use_fonthost_mac=false`, so force-loading
the archive exposed CoreText imports. The gate applies a checksum-locked patch
to a symlink-based disposable Skia source view. The patch removes exactly those
two utility TUs when GN selects macOS while `skia_use_fonthost_mac=false`.
Keeping Skia's Darwin feature selection is important for correct macOS malloc
and threading semantics; pretending the compiler target is Linux or Android
would incorrectly select `<malloc.h>` and Bionic/ELF paths.

The GN module enables `skia_enable_fontmgr_custom_empty` and
`skia_include_multiframe_procs`, and `skia_enable_android_utils`, so upstream
`src/ports/SkFontMgr_custom_empty.cpp` and `tools/SkSharingProc.cpp` are real
members of `libskia.a`, alongside the Android framework's real
`BitmapRegionDecoder.cpp` and `FrontBufferedStream.cpp`. The latter two add six
definitions consumed by GraphicsJNI; the gate audits their exact mangled-name
manifest rather than filling those symbols separately. The resulting archive
has 523 members. The sharing implementation's PNG deserialize path is built
against the pinned AOSP libpng/zlib archives. No symbol is synthesized after
the build. Apple's ImageIO/CoreGraphics remain system image providers; CoreText
is neither linked nor imported.
The passing gate emits locked manifests for the six Android-utils symbols and
for `CoreText=0`/`Homebrew=0`; both the archive and final executable are
checked, so enabling Android utils cannot silently regress the Darwin provider
boundary.

Run:

```sh
tools/build-android16-skia-hwui-force-load.sh
```

The gate force-loads the complete Skia archive into an arm64 executable,
requires the pinned AOSP FreeType, libpng, zlib, liblog, and libcutils archives
in the link map, executes the empty font-manager/sharing-context plus Android
front-buffer/invalid-region-decoder smoke test,
and rejects CoreText symbols,
CoreText dylibs, Homebrew paths, and host FreeType/png/zlib dylibs through
`nm`, the link map, and `otool -L`.
