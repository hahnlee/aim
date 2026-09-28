# Bionic binary stdio facade

This standalone provider implements the coherent fixed-register binary slice of
the pinned libc++ stdio imports. Android `FILE` is a 152-byte, 8-byte-aligned
opaque token and is never reinterpreted as Darwin `FILE*`. Dynamic tokens own no
host pointer; a mutex-protected side table owns all cursor, bytes, mode, pushback,
and virtual-fd state.

`__sF` is a guest-visible array of exactly three permanent Android FILE tokens.
During activation they represent an in-memory stdin source and captured
stdout/stderr sinks with virtual descriptors 0/1/2. Their storage has process
lifetime, while operations require an active provider. Dynamic `fopen` tokens
are freed by `fclose`; stale use is never dereferenced and returns `EBADF` until
allocator reuse, after which POSIX stale-FILE use remains undefined.

Supported central imports are `fopen`, `fclose`, `fflush`, `fileno`, `fread`, `fwrite`,
`fseek`, `fseeko`, `ftello`, `fputc`, `getc`, and one-byte `ungetc`. The runtime
extension resolver also provides `fgetc` and `fgets` over the same cursor. Paths and
modes are byte strings. This vertical slice uses explicit in-memory file
snapshots and private writable streams, so it cannot expose host paths, FILEs,
or descriptors. `fileno` values starting at 20000 are provider-local only.
The accepted mode grammar is deliberately narrow: `r` or `w`, followed by at
most one each of `+`, `b`, and Bionic's `e` close-on-exec modifier in any order.
The provider already owns the descriptor, so `e` changes no host state but must
be accepted for Bionic libc++ `basic_filebuf`. Append, persistence after close,
and host filesystem access are unsupported rather than silently approximated.
The standalone in-memory backing is capped at 16 MiB per stream; writes or
sparse extensions beyond that return Android `EFBIG` with checked arithmetic.

Long-term integration must replace the backing store and virtual-fd allocator
with the central filesystem facade namespace. A stdio token must retain one
reference to that central open-file description so `FILE` buffering, fd close,
and dup semantics have one owner; these local virtual fds must never be passed
to the current filesystem provider.

`fprintf`/`vfprintf` and all formatted varargs remain absent from the central
resolver. The separately pinned wide-stdio owner supplies `fputwc`, `getwc`,
and `ungetwc`; it never reinterprets Android `FILE` or `wchar_t` as a Darwin
type. Activation installs an explicit callback ABI into that owner. One
exclusive central stream lease is held for each complete wide call, so byte
operations, another wide conversion, close, and seek cannot interleave with a
partial UTF-8 sequence. The exact central classification remains in
`manifests/imports.tsv`; generated namespace ownership assigns the three wide
imports only to `wide-stdio`.

Close first acquires the exclusive lease, drains a current wide operation,
forgets the wide side-table state, and only then removes and frees the 152-byte
token. Seek performs reset under the same exclusive lease before changing the
cursor. Process uninstall rejects new leases, drains all current leases,
forgets every live token, and only then uninstalls the wide callback table.
This ordering prevents stale `mbstate_t` and pushback from surviving allocator
address reuse. Runtime users call the refcounted process install/uninstall C
ABI; standalone probes use the same activation path.

The global table lock serializes each operation including concurrent close/use.
Whichever acquires it before close completes; later calls return `EBADF` without
dereferencing the freed token. The gate runs this contract from real Android
AArch64 code, including UTF-8 wide I/O and concurrent get/seek/close. The
C/C++ boundary runs the same E2E under ASan, UBSan, and TSan; the ownership
core remains safe Rust with checked arithmetic. Clippy, formatting, exact
imports, source-pinned ABI probes, and target-clean checks complete the gate.
Allocation exhaustion inside the safe Rust backing store is not translated to
Android `ENOMEM`; the fixed 16 MiB cap bounds individual stream growth, but a
future central filesystem owner must provide fallible allocation/accounting.
