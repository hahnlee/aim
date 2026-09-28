# Android arm64 scanf facade

This standalone gate owns `sscanf@LIBC`, `vsscanf@LIBC`, and `fscanf@LIBC`.
It does not forward a Darwin `va_list`, `FILE`, `scanf`, wide-character API,
or `long double` value.

`sscanf` enters through an AAPCS64 assembly capture. `vsscanf` reads the exact
32-byte Android arm64 `va_list` (`stack`, `gr_top`, `vr_top`, `gr_offs`,
`vr_offs`). Scanned arguments are pointers, so only the GP cursor advances;
the acceptance fixture deliberately fills the FP bank while spilling pointer
arguments to prove that separation.

The parser is a string-reader port of the pinned Android 16 Bionic
`vfscanf.cpp`/`scanf_common.h` conversion model. It supports integer, binary,
float, byte/UTF-8-to-wchar32 string, character, scanset, `%n`, widths,
suppression, `hh/h/l/ll/j/z/t/L`, deprecated `q`, and Bionic `w/wf` integer
widths. Bionic `%m` allocation is outside the pinned libc++ corpus and is
rejected with Android `ENOTSUP`; there is no partial allocation success.
Unknown or malformed formats likewise return `-1` and set Android errno.

`fscanf` uses a second AAPCS64 assembly capture and calls the same parser over
the unread snapshot of a provider-owned Android `FILE`. The stdio facade holds
its central stream lease for the scan and advances the cursor by the parser's
exact consumed-byte count. A Darwin `FILE*` never crosses this boundary.

`%Lf` calls the existing binary128 raw conversion seam, which uses AOSP gdtoa
and writes IEEE binary128 bytes. Darwin `long double` never enters the C++
implementation. The numeric, float, binary128, allocator, errno, and ICU
archives remain separate sole owners and must follow this archive in static
link order.

Run `bash tools/bionic-scanf-facade/audit.sh`. The resulting archive is
`_build/bionic-scanf-facade/libaim-bionic-scanf.a`.
