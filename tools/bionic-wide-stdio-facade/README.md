# Bionic wide stdio facade

This standalone provider closes the three Android arm64 libc imports that the
pinned NDK r28c libc++ needs from Bionic's wide-stream layer: `fputwc`, `getwc`,
and `ungetwc`. It deliberately does not reinterpret Android's unsigned
32-bit `wchar_t` or 152-byte `FILE` as Darwin `wchar_t`/`FILE`.

## Ownership and semantics

The source lock pins Android 16 Bionic revision
`361ba86734fb2821a6adcfdf775db8abd04e0de0`. `getwc` is the thin checked alias
in `libc/stdio/stdio.cpp`; the algorithms are the pinned OpenBSD-derived
`fgetwc.c`, `fputwc.c`, and `ungetwc.c`, with state layout and the one-wide-char
pushback bound from `libc/stdio/local.h`.

The facade keeps the two eight-byte Android `mbstate_t` values and one-element
wide pushback in a provider side table. UTF-8 conversion is delegated to the
already-owned `bionic-locale-facade`; this module does not duplicate conversion
logic or call Darwin wide-character APIs. Consequently it preserves Bionic's
asymmetric edge behavior: UTF-8 input encoding a surrogate is `EILSEQ`, while
`fputwc(0xd800)` emits `ed a0 80`, as pinned `c32rtomb.cpp` permits values up to
`0x1fffff`. EOF after an incomplete input sequence is plain `WEOF`: the byte
owner sets EOF, but pinned `fgetwc.c` neither synthesizes `EILSEQ` nor sets the
stream error bit. Pinned `fputwc.c` clears `wcio_ungetwc_inbuf` before encoding.

## Central stdio integration boundary

`AimBionicWideStdioBackendV1` is the only connection to the central stdio
owner. `acquire` validates an opaque Android `FILE*` token and returns a lease
that keeps the stream live and exclusively locked until `release`. The remaining
callbacks operate on that lease and share the central orientation, EOF, error,
and byte I/O state. A read callback returns exactly one byte (`1`), EOF (`0`),
or failure (`-1`); a write callback succeeds only after writing all bytes.
Callback failures store Android TLS errno themselves.

The lock order is always central stream lease, then the short-lived state-map
mutex, then that stream's own wide-state mutex. I/O does not hold the map mutex,
so a blocked stream cannot serialize an unrelated stream.
Close and seek/reset must already hold the central exclusive stream lease when
they call `aim_bionic_wide_stdio_forget` or
`aim_bionic_wide_stdio_reset`. They must not call either hook from a
facade callback. Close must finish `forget` before the 152-byte token can be
reused for a new stream generation; otherwise stale `mbstate_t` or pushback
could be attached to the recycled address. All streams must be forgotten before
the process-scoped backend is uninstalled. The sanitizer stress fixes this
contract with independent-stream progress, concurrent read/close and read/reset
serialization, reset after incomplete UTF-8, and same-address token reuse.

`orient_wide` follows Bionic `_SET_ORIENTATION(fp, 1)`: it changes an
unoriented stream to wide orientation and does not manufacture a rejection for
an already byte-oriented stream. The future central owner remains responsible
for applying the matching byte-operation orientation rules.

## Gates

Run:

```sh
tools/bionic-wide-stdio-facade/audit.sh
```

The audit re-censuses the exact three libc++ imports, downloads and hashes only
the pinned Bionic owner sources, checks API-35 `@@LIBC` exports and Android FILE
ABI, builds an Android arm64 ELF whose undefined namespace is exactly the three
functions plus `__errno`, loads it through the closed ELF resolver, and runs
ASan/UBSan stress for UTF-8, invalid/surrogate behavior, EOF, pushback ordering,
eight-thread locking, close/reset races, token reuse, and Darwin errno
preservation. It links only the pinned locale archive and static ICU 76.1
foundation; host/dynamic ICU and Darwin wide stdio are forbidden.

This directory intentionally does not integrate the provider into the existing
stdio, namespace, runtime, or loader owners. That composition remains a later
central-owner change using the documented callback ABI.
