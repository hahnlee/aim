# Android 16 zlib, libpng, and FreeType Darwin gate

The exact projects, peeled Android 16 commits, Android.bp hashes, and public
header hashes are recorded in `android16-graphics-codecs.lock`. The build gate
does not clone Git repositories and does not implement unresolved symbols.

## Required gitless materialization

Before running the build with its default paths, materialize these complete
Gitiles archives:

- `platform/external/zlib` at the locked revision into `_aosp/external/zlib`;
- `platform/external/libpng` at the locked revision into
  `_aosp/external/libpng`;
- `platform/external/freetype` at the locked revision into
  `_aosp/external/freetype`.

Use commit-addressed `+archive/<revision>.tar.gz` URLs, extract through a
temporary sibling directory, verify both locked hashes, write
`.source-revision`, and atomically rename the directory into place. Do not
materialize `.git` metadata. Gitiles archive bytes themselves are not pinned
because regenerated tar metadata is not stable.

If any subtree is absent, `tools/build-android16-graphics-codecs.sh` reports its
project, subtree, revision, and Android.bp hash together and exits with status
2. `AIM_ANDROID16_EXTERNAL_ROOT` may point at a separately materialized
verification root with the same `zlib`, `libpng`, and `freetype` layout.

## Android.bp source selections

- `libz`: all 19 entries in `libz_srcs`, shared flags, arm64 flags, and the
  `target.darwin_arm64` `ARMV8_OS_MACOS` definition.
- `libpng`: root `*.c` except `example.c` and `pngtest.c`, plus the three arm64 C
  implementations; `arm/filter_neon.S` remains excluded exactly as Android.bp
  specifies. Total: 18 translation units.
- `libft2`: all 26 `libft2_defaults.srcs`, PNG and system-zlib feature defines,
  and `target.not_windows` PIC flags.

The gate emits three arm64 archives under `_build/graphics-codecs`, verifies
the exact member manifests, and requires representative inflate/deflate, PNG
read/write, and FreeType face/glyph API definitions from upstream objects.
It then loads the checksum-locked Roboto Regular test font through those actual
archives, rasterizes glyph `C` at 32 pixels, and verifies its bitmap and metrics
with a stable hash.
