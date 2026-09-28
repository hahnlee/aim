# Bionic `strftime_l` facade

This standalone provider closes the one `strftime_l@LIBC` import in the pinned
NDK r28c API 35 arm64 `libc++_shared.so`. It compiles the hash-locked Android 16
Bionic `libc/tzcode/strftime.c` rather than forwarding an Android `struct tm` or
`locale_t` to Darwin. The exported resolver owns only the exact tuple
`(libc.so, strftime_l, LIBC)` and has no `dlsym`, dyld, or host-libc fallback.

## Why this is the complete format surface

The binary contains 17 direct callsites. Fourteen initialize C-locale weekday,
month, and AM/PM tables, two `time_get` specializations construct a conversion
at runtime, and the `time_put` path constructs `%[modifier]conversion` from its
facet arguments. Consequently a short constant-format allowlist would reject
valid pinned-libc++ behavior. `manifests/formats.tsv` records the exact full
surface supplied by the pinned AOSP source, including Android `_`, `-`, `0`,
`^`, `#`, and `%P` behavior.

The locale argument is deliberately ignored. This is not a Darwin shortcut:
the pinned Bionic source implements only `C_time_locale`, and its `strftime_l`
entry is an exact call to `strftime` with the locale unused.

## Timezone ownership boundary

Formatting is admitted only while a process-wide immutable timezone owner is
active. Activation copies standard/daylight abbreviations and fixed offsets
(seconds east of UTC); deactivation stops admission and waits for all in-flight
calls. `%Z` still follows Bionic's `tm_zone`-then-`tzname` behavior and `%z`
uses `tm_gmtoff` exactly.

This module does **not** bundle Android tzdata or the full tzcode transition
engine. `%s` is exact for an explicitly selected standard/daylight fixed
offset. When `tm_isdst < 0` would require choosing between unequal offsets, the
facade fails closed with Bionic `EOPNOTSUPP` instead of guessing a transition.
IANA-zone activation remains a separate timezone-provider integration seam.

All failures are written to the provider-owned Bionic errno TLS. Darwin errno
is restored on every path. Timezone names are immutable owned copies, and the
only process-global state is protected by a mutex plus quiescent drain.

## Audit

Run:

```sh
tools/bionic-strftime-facade/audit.sh
```

The gate hash-locks the NDK, the exact Bionic/LLVM inputs, the import and all 17
disassembly callsites; builds an Android AArch64 ELF importing only
`strftime_l@LIBC`; rejects host formatting/timezone fallbacks; and executes the
fixture through the ART ELF loader under ASan and UBSan, including the complete
C-locale surface, Android modifiers, errno behavior, lifecycle, and concurrent
threads.
