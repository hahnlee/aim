# Android 16 Skia FreeType text-raster gate

This is an additive text build. It leaves the existing CPU/no-font
`_build/skia` baseline untouched and generates `_build/skia-text` from the same
revision-locked Android 16 Skia source.

The gate enables the FreeType typeface and custom in-memory font manager while
explicitly disabling the empty, CoreText, Android-NDK, directory, and
fontconfig managers. Skia's upstream `SkUserConfigManual.h` defines
`SK_BUILD_FOR_ANDROID_FRAMEWORK`, and the GN system-FreeType target receives
the absolute path to the pinned AOSP
FreeType headers and `_build/graphics-codecs/libft2-darwin.a`. The executable is
then linked explicitly with that archive and the pinned AOSP libpng/zlib
archives. Android framework configuration and logging use Skia's upstream
`SkUserConfigManual.h` plus pinned AOSP liblog headers/archive; the repository's
`compat/` headers and symbols are not on this build's include or link path. No
`-lfreetype`, Homebrew path, CoreText framework, compatibility symbol stub, or
host codec library is permitted.

Run prerequisites and the gate with:

```sh
tools/build-android16-graphics-codecs.sh
tools/build-android16-skia-text-raster.sh
```

The script verifies Skia/FreeType manifests and tool hashes, GN's resolved
`//third_party/freetype2:freetype2.libs`, archive architecture and representative
symbols, the final link map, and `otool -L`. Its executable loads the pinned
Roboto Regular TTF through `SkFontMgr_New_Custom_Data`, maps all five `Click`
glyphs, rasterizes them through `SkCanvas` into a CPU `SkSurface`, checks that
non-background glyph pixels exist, and compares a deterministic full-pixel
hash. The accepted Android 16 arm64 result is five glyphs, 1,097 ink pixels,
and FNV-1a pixel hash `1f94df6816828ca2`.

Skia's current GN accepts an absolute archive path in
`skia_system_freetype2_lib`; therefore no upstream patch is expected. If that
contract changes, the gate exits 2 and proposes the narrow fix: introduce a
path-valued `skia_system_freetype2_archive` argument in
`third_party/freetype2/BUILD.gn` and append it to the system target's `libs`.
The gate must not fall back to a library name or add a symbol stub.
