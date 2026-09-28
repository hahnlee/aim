# Android 35 Bionic locale/multibyte facade

This standalone Darwin arm64 provider owns 31 of the exact 32 Class-C
locale-related imports in the pinned NDK r28c/API 35 arm64
`libc++_shared.so`. It implements one coherent state boundary rather than
forwarding Android objects to similarly named Darwin functions.

The supported owner contains:

- `newlocale`, `freelocale`, `uselocale`, `setlocale`, `localeconv`, and
  `__ctype_get_mb_cur_max`;
- `btowc`, `wctob`, `mbrlen`, `mbrtowc`, `mbtowc`, `mbsrtowcs`,
  `mbsnrtowcs`, `wcrtomb`, and `wcsnrtombs`;
- Bionic's C-locale `strcoll_l`, `strxfrm_l`, `wcscoll_l`, and `wcsxfrm_l`.
- all ten libc++-imported `isw*_l` functions plus `towlower_l` and
  `towupper_l`, backed by the source-built Android ICU 76.1 common archive
  and the locked `icudt76l.dat`.

Only the names Bionic itself accepts are supported: `C`, `POSIX`, `C.UTF-8`,
`en_US.UTF-8`, and the empty environment-selection name, which Bionic maps to
UTF-8 without consulting the host environment. Unknown names fail with Android
`ENOENT`; invalid categories and masks fail with Android `EINVAL`.

## State and ABI boundary

Android `locale_t` is represented by provider-owned immutable entries in a
locked side table. `uselocale` stores only the provider handle and mode in
Darwin thread-local state; the all-ones Android `LC_GLOBAL_LOCALE` sentinel is
handled explicitly. No Android handle is cast to Darwin `locale_t`, and the
provider never calls host `setlocale`, `newlocale`, or multibyte routines. The
process locale is one atomic UTF-8/C mode, while explicit thread locales remain
stable across concurrent process-mode updates.

Android arm64 `mbstate_t` is exactly eight bytes with the first four bytes
holding Bionic's partial UTF-8 sequence. The provider implements the pinned
`mbrtoc32`/`c32rtomb` algorithms directly, including incomplete `-2`, illegal
`-1` plus Android `EILSEQ`, reset-after-error, overlong/surrogate decode
rejection, and Bionic's historical encoder range through `0x1fffff`. Null-state
conversion storage is thread-local rather than a Darwin `mbstate_t`.

Android arm64 uses unsigned 32-bit `wchar_t`; collation and transformation use
explicit `uint32_t` comparison/copy loops. This preserves ordering above
`INT32_MAX` and avoids passing Android wide strings to Darwin wide APIs.
`localeconv` exposes the Android 96-byte layout and unsigned-char `CHAR_MAX`
bytes, not Darwin's `struct lconv`.

The wide classification functions reproduce the property selection in the
pinned Bionic `wctype.cpp`: `UCHAR_ALPHABETIC`, `UCHAR_POSIX_BLANK`, ICU
control/digit/punctuation predicates, `UCHAR_LOWERCASE`, `UCHAR_POSIX_PRINT`,
`UCHAR_WHITE_SPACE`, `UCHAR_UPPERCASE`, and `UCHAR_POSIX_XDIGIT`. Mapping uses
ICU simple case conversion after Bionic's ASCII fast path. Android `wint_t`
values are converted through ICU's signed `UChar32`, so `WEOF`, surrogates,
noncharacters, out-of-range values, and values above `INT32_MAX` follow ICU
76.1 rather than Darwin tables. As in AOSP, each `_l` function deliberately
ignores its `locale_t` argument; null, global, stale, and arbitrary values are
therefore never dereferenced and have identical Unicode semantics.

The provider initializes the Android ICU data owner once, validates runtime
version 76.1, and links only `_build/icu-foundation`'s static common,
stubdata, and whole-loaded `libandroidicuinit` archives. It has no host or
dynamic ICU fallback. `ANDROID_I18N_ROOT` must name a runtime tree containing
`etc/icu/icudt76l.dat`, as it does for the rest of this port.

All provider calls save and restore Darwin pthread errno and floating-point
environment. Capability failures are written only to the existing standalone
Bionic errno TLS owner. The locale archive deliberately leaves that provider
as an unresolved integration dependency, preventing a duplicate `__errno`
owner.

## Explicitly unsupported

`strftime_l` is excluded because its implementation is
the full Bionic tzcode `strftime` owner, including timezone state and Android
format extensions; forwarding to Darwin would overclaim compatibility.

Thus the closed resolver exposes exactly 31 `libc.so@LIBC` symbols. It has no
`dlsym`, dyld, unprefixed interposition, host-global locale mutation, or fallback
for the remaining 13 imports.

## Provenance and acceptance

Run:

```sh
tools/bionic-locale-facade/audit.sh
```

The gate sparsely materializes 21 SHA-locked Bionic implementation/header files
without Git metadata and verifies the Android 16 ICU foundation lock, headers,
three static archives, and 29,094,000-byte data file. It derives the 32-symbol
cluster from the canonical 160-import manifest, checks the 31/1 partition,
validates Android
`locale_t`/`mbstate_t`/`wchar_t`/`lconv` layouts, and builds a real NDK AArch64
ELF importing all 31 functions plus `__errno`. The loaded fixture exercises
locale aliases and failures, UTF-8 partial/invalid conversion and source-pointer
rules, unsigned wide collation, Unicode property/mapping edges, and eight
concurrent thread locales. A direct differential compares all 12 wide APIs
against ICU 76.1 for ASCII, BMP, supplementary, surrogate, noncharacter,
out-of-range, signed-boundary, and `WEOF` inputs under four ignored locale
values. A separate ASan/UBSan stress performs 800 locale lifecycles and
concurrent ICU calls, and requires zero live handles.
Host errno and fenv preservation are checked across the full Android ELF run.

Artifacts are written to `_build/bionic-locale-facade`.
