# Android 16 HarfBuzz Darwin ARM64 foundation gate

This gate builds the complete `libharfbuzz_ng` host source selection from the
Android 16 `external/harfbuzz_ng/Android.bp`. It is a foundation for Minikin and
therefore for framework text measurement, glyph shaping, and the first
`TextView`/`Button` rendering path. It is not a reduced symbol-provider archive.

## Source identity

`android16-harfbuzz.lock` pins AOSP project
`platform/external/harfbuzz_ng` at revision
`e489c416b6f8d2a9a2e0e85b781d1e4a0c431401`. The verifier checks all of:

- the exact `Android.bp` hash;
- all 53 translation units selected by the `libharfbuzz_ng` module, in Blueprint
  order, including their content hashes;
- all 459 files under `src/`, including headers and generated upstream tables,
  using a sorted content manifest.

Run the source gate with:

```sh
tools/verify-android16-harfbuzz-source.sh
```

`HARFBUZZ_SOURCE_ROOT` may point the verifier at a separately extracted Gitiles
archive. Missing source is a materialization blocker and returns exit status 2
with the exact AOSP project, revision, destination, and Gitiles URLs. An identity
mismatch returns status 3; it is never treated as a compatible source update.

## Module selection and compile contract

The build script parses the pinned `libharfbuzz_ng.srcs` list directly from
`Android.bp`; it does not maintain a hand-selected source subset. The Darwin
ARM64 archive uses C++20, `-arch arm64`, PIC, hidden default visibility, and the
module's complete compile definitions and warning policy:

```text
HAVE_PTHREAD
HB_NO_PRAGMA_GCC_DIAGNOSTIC
HAVE_OT
HAVE_ICU
HAVE_ICU_BUILTIN
-Werror
-Wno-unused-parameter
-Wno-missing-field-initializers
-Wno-implicit-fallthrough
```

The Android.bp host closure is preserved as modules:

```text
libharfbuzz_ng
  shared: libicu, liblog
  static (host): libcutils, libutils
```

No per-symbol stubs, locally reimplemented ICU entry points, or source-pruned
substitute archives are allowed. Darwin-specific work must remain at module or
platform-adapter boundaries.

## Gates

To build and audit the full 53-object ARM64 archive before ICU is available:

```sh
tools/build-android16-harfbuzz-foundation.sh --archive-only
```

The archive gate checks the architecture, exact member count, and representative
public definitions from the buffer, OpenType layout, shaping, and Unicode paths.

The default command additionally force-loads the entire HarfBuzz archive into a
small executable and runs shaping/Unicode smoke assertions:

```sh
tools/build-android16-harfbuzz-foundation.sh
```

It requires module-complete Darwin ARM64 archives for the direct Android.bp
dependency `libicu`, its `libicuuc`/`libicui18n` implementation closure, and
`liblog`, `libcutils`, and `libutils`. Until the ICU foundation gate exists, the
command intentionally reports every missing archive and exits 2 after the
HarfBuzz archive audit. Locations can be overridden with
`HARFBUZZ_LIBICU_ARCHIVE`, `HARFBUZZ_ICUUC_ARCHIVE`,
`HARFBUZZ_ICUI18N_ARCHIVE`, `HARFBUZZ_LIBLOG_ARCHIVE`,
`HARFBUZZ_LIBCUTILS_ARCHIVE`, and `HARFBUZZ_LIBUTILS_ARCHIVE`.

The executable smoke is a foundation test, not the text-rendering acceptance.
The next acceptance is Minikin shaping a known UTF-8 string with a pinned font;
the framework acceptance after that is a `TextView`/`Button` layout producing
glyph runs consumed by HWUI/Skia.
