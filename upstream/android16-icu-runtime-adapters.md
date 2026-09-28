# Android 16 ICU runtime adapter gate

This gate compiles the existing aim compatibility sources
`compat/darwin_icu_natives.cc` and `compat/darwin_libcore_natives.cc` as
separate Mach-O arm64 objects against the sparse Android 16 ICU76 headers.
The sources are checksum-locked and are not copied, rewritten, or patched.

The include order is deliberate: `android_icu4c/include` and
`icu4c/source/common` precede all host paths, so `unicode/urename.h` maps the
adapter calls to the `_76` ABI. The two objects must expose exactly the locked
50-symbol ICU76 undefined closure and zero `_78` symbols before linking.

The executable links only the module-complete archives produced by
`tools/build-android16-icu-foundation.sh`: common 201, i18n 254, stubdata 1,
and whole-static `libandroidicuinit` 2. Homebrew and system ICU libraries are
forbidden.

The locked `icudt76l.dat` is staged at:

```text
_build/icu-runtime-adapters/runtime/i18n/etc/icu/icudt76l.dat
```

The smoke process runs with `ANDROID_I18N_ROOT` pointing at that runtime,
calls upstream `android_icu_init()`, and requires both the linked ICU library
and registered common-data version to be 76. It additionally opens the UTF-8
converter used by `darwin_icu_natives` and resolves a Unicode character name
used by `darwin_libcore_natives`.

Run:

```sh
tools/build-android16-icu-runtime-adapters.sh
```
