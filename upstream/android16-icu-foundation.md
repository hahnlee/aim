# Android 16 Darwin ICU foundation gate

This gate builds the complete Darwin host variants of the pinned Android 16
ICU modules. It does not use Homebrew ICU, replace missing APIs, or patch the
upstream sources.

## Gitless source contract

Do not download or retain the full ICU repository archive. Materialize only
the following paths from
`platform/external/icu@f17caeafcf20bd38074a9963c31df3629b70b5f5` into
`_aosp/external/icu-graphics`:

- locked individual files: `Android.bp`, `android_icu4c/Android.bp`, and
  `icu4c/source/Android.bp`;
- commit-addressed Gitiles subtree archives: `android_icu4c/include`,
  `icu4c/source/common`, `icu4c/source/i18n`, `icu4c/source/stubdata`, and
  `libandroidicuinit`.

These selected paths occupy about 49 MiB extracted, including the 29 MiB data
file; the full extracted project is roughly 425 MiB and is explicitly outside
the materialization contract. Tests, tools, ICU4J, generated data inputs, and
all other subtrees are excluded.

Fetch and base64-decode individual files through the commit-addressed Gitiles
`?format=TEXT` endpoint, and fetch subtree archives through
`+archive/<revision>/<subtree>.tar.gz`.
Build the layout in a temporary sibling directory, verify the locked
Android.bp, source-manifest, config-header, and data hashes, write
`.source-revision`, then atomically rename it into place. The destination must
not contain `.git`.

This separate graphics root avoids conflating the ART bootstrap's partial ICU
header input with the full text runtime closure. Every selected source and
every defining Android.bp is part of the gate.
`AIM_ANDROID16_ICU_ROOT` can point at a sparse external
materialization without changing the repository checkout. Missing or partial
materializations exit with status 2 and report the project, revision, subtree,
and expected hash.

## Module mapping

The source selection and flags follow the pinned Android.bp files:

- `libicuuc`: all 201 `icu4c/source/common/*.cpp` files, ICU common defines,
  hidden visibility, O3, and RTTI;
- `libicui18n`: all 254 `icu4c/source/i18n/*.cpp` files, ICU i18n defines,
  hidden visibility, O3, and RTTI;
- `libicuuc_stubdata`: `stubdata/stubdata.cpp`;
- `libandroidicuinit`: both C++ files and its host logging implementation.

For the non-Windows host variant, `libicuuc_stubdata` is a static dependency
and `libandroidicuinit` is a whole-static dependency. The smoke executable
models that relationship with `-force_load` and verifies that both init object
files survive linking. The archive member manifests must contain exactly
201, 254, 1, and 2 upstream objects.

## Runtime data

The gate stages the locked 29,094,000-byte `icudt76l.dat` as:

```text
_build/icu-foundation/runtime/i18n/etc/icu/icudt76l.dat
```

The executable runs with `ANDROID_DATA`, `ANDROID_TZDATA_ROOT`, and
`ANDROID_I18N_ROOT` set. It explicitly exercises the upstream host
`libandroidicuinit` mmap registration path, ICU 76 common-data lookup,
Korean number formatting, and word breaking before cleanup. Absence of the
optional timezone resource directory is logged by upstream code; it does not
replace or weaken the common-data assertion.

Run:

```bash
tools/build-android16-icu-foundation.sh
```

The resulting executable and all four archives must be Mach-O arm64, and the
executable must not dynamically load a Homebrew or system ICU library.
