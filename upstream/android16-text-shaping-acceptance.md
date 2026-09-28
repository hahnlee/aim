# Android 16 Darwin text shaping acceptance

## Scope

The first executable should prove a real Android text path, but stop before
HWUI and Skia rasterization:

```text
pinned TTF
  -> upstream FreeType MinikinFont adapter
  -> Minikin Font / FontFamily / FontCollection
  -> Minikin Layout
  -> HarfBuzz OpenType shaping
  -> ICU bidi, script, locale, and break data
  -> glyph IDs, clusters, positions, bounds, and advances
```

This is smaller and more diagnostic than asking Skia to draw immediately. It
does not permit a fake `MinikinFont`, a host-installed ICU, CoreText, a system
font, or a per-symbol shim. The adapter is the upstream
`tests/util/FreeTypeMinikinFontForTest.cpp`, backed by the pinned AOSP FreeType
module.

## Revision and module closure

| Project | Revision | Darwin module source count |
|---|---|---:|
| `platform/frameworks/minikin` | `1e1d5d137d487df875d7db69b5ff24e7d0291612` | `libminikin`: 31 C++ |
| `platform/external/harfbuzz_ng` | `e489c416b6f8d2a9a2e0e85b781d1e4a0c431401` | `libharfbuzz_ng`: 53 C++ |
| `platform/external/icu` | `f17caeafcf20bd38074a9963c31df3629b70b5f5` | `libicuuc`: 201 C++, `libicui18n`: 254 C++ |
| `platform/external/icu` | same | `libicuuc_stubdata`: 1 C++, `libandroidicuinit`: 2 C++ |
| `platform/external/freetype` | `d968d2541f7158e18ab22680bfa08a538019bf6a` | `libft2`: 26 C |
| `platform/external/skia` | `bcb0f77c44783b1800ba37641ba7ecab04f05e07` | subsequent raster gate |

The exact 31, 53, and 26 source manifests and the Android.bp hashes are
enforced by `tools/check-android16-text-shaping-inputs.sh`. Minikin's generated
Rust bridge is not a Darwin runtime dependency: `Hyphenator.cpp` includes and
calls it only under `__linux__`. It must not be replaced with a Darwin stub.

`tools/build-android16-minikin-foundation.sh` already compiles the complete
31-source Darwin release variant into an arm64 Mach-O archive. It records the
remaining HarfBuzz/ICU/platform undefined closure instead of satisfying it
with local definitions. The source directory must not be added as a general
`-I` path: its private `locale.h` otherwise shadows the macOS SDK header while
libc++ parses `<locale>`; quoted private includes resolve relative to each
Minikin source without that path.

The first shaping executable can use upstream `libft2.nodep`, which is a real
FreeType module variant and avoids introducing PNG/zlib into a glyph-metrics
test. Production Skia rasterization must use full `libft2` with the pinned
`libpng` and `libz` closure.

## Deterministic fonts

The first gate uses fonts already owned by the Minikin revision:

| Fixture | SHA-256 | Purpose |
|---|---|---|
| `Ascii.ttf` | `3747ed19af40728701dc2c1accc0684fd6c2c72dba08f3f96263269e0846cffe` | Latin layout and advances |
| `Arabic.ttf` | `dce476b160ce641d424a1d03216d6c541ffd768115dbcd665ef0e425e711d5b7` | RTL and Arabic shaping |
| `Hangul.ttf` | `f51078f1915c63440334e2c61290e1461f26b6661151eb6cba3cd81e749fbb9f` | non-Latin fallback/script coverage |

These fixtures are intentionally preferable to `/System/Library/Fonts` or a
Homebrew font. A later visual Button gate should pin Android Roboto project
revision `c50938f329a44707b06b336166c95ec2aa49c331`; it is not required to prove
the first shaping boundary.

## ICU host data contract

The host `libicuuc` whole-archives `libandroidicuinit` and requires
`icu-data_host_i18n_apex`. The pinned data file is:

```text
icu4c/source/stubdata/icudt76l.dat
sha256 c5450087565eb20ca37d70af5ef53a99a4c8e2e3da17c9140582b685f06d980f
size   29094000 bytes
```

The acceptance runner should stage it as:

```text
_build/text-shaping-acceptance/runtime/i18n/etc/icu/icudt76l.dat
```

and launch with all three variables set:

```bash
ANDROID_DATA=_build/text-shaping-acceptance/runtime/data
ANDROID_TZDATA_ROOT=_build/text-shaping-acceptance/runtime/tzdata
ANDROID_I18N_ROOT=_build/text-shaping-acceptance/runtime/i18n
```

All three variables are required before the host constructor invokes Android
ICU initialization. `ANDROID_I18N_ROOT/etc/icu/icudt76l.dat` is mandatory and
failure to map or register it aborts. The timezone path is
`$ANDROID_TZDATA_ROOT/etc/tz/versioned/9/icu`; its absence is logged but does
not replace the common ICU data. The executable must call ICU cleanup only
after every Minikin/HarfBuzz object has been destroyed.

## Executable assertions

Construct a `FreeTypeMinikinFontForTest`, then `Font::Builder`, `FontFamily`,
`FontCollection`, and `MinikinPaint`. For each input construct a real
`minikin::Layout` and assert:

1. `nGlyphs()` is non-zero and every glyph ID is valid for its chosen face.
2. `getAdvance()` is positive and equals the sum of character advances within
   a small float tolerance.
3. Glyph clusters are inside the UTF-16 input range.
4. Bounds returned through the FreeType adapter are non-empty.
5. Latin `Click`, Arabic text in RTL mode, and Hangul each produce a font run.
6. A feature-bearing input is shaped through HarfBuzz rather than copied as
   UTF-16 code units; record the deterministic glyph/cluster/position digest.
7. The executable and all archives are arm64 Mach-O and `otool -L` contains no
   Homebrew ICU, FreeType, or HarfBuzz.

The acceptance should print a digest per input, ICU version/data name, glyph
count, run count, and total advance. Checking only link success or calling
`hb_shape` directly is insufficient: the goal is to exercise Minikin's
selection, caching, bidi, and layout path.

## Skia and FreeType raster follow-up

The separate `_build/skia-text/args.gn` must contain at least:

```gn
skia_use_freetype = true
skia_enable_fontmgr_empty = false
skia_use_system_freetype2 = true
skia_system_freetype2_include_path = "<repo>/_aosp/external/freetype/include"
```

The GN link input must resolve to the pinned AOSP `libft2` archive, not macOS,
Homebrew, or an implicit `-lfreetype`. For the production-like raster gate use
full `libft2` with its pinned PNG/zlib dependencies. Keep Skia and HWUI RTTI
off, while ICU retains the `rtti: true` declared by both ICU modules.

The raster acceptance then extends the shaping executable:

```text
Minikin Layout glyph IDs/positions
  -> FreeType-backed SkTypeface
  -> SkFont / SkCanvas::drawGlyphs
  -> CPU SkSurface
  -> non-background pixel count and deterministic pixel hash
```

Only after this passes should the same glyph buffer be connected to the HWUI
Canvas and macOS window bridge.

## Current materialization boundary

Minikin, its deterministic fonts, HarfBuzz, FreeType, zlib, libpng, and public
ICU common headers are now revision-locked and materialized without Git
history. The complete Minikin archive builds as 31 arm64 objects and retains
215 unique undefined dependency symbols; the combined platform/HarfBuzz
closure leaves the real ICU dependency. Full ICU common/i18n sources,
data/init, and the separate Skia FreeType build remain. The readiness script
reports those exact path/count/hash and GN-argument blockers. It does not
download unpinned fallbacks or manufacture substitutes.
