# Android 16 OsConstants Darwin compatibility owner

`android.system.OsConstants` exposes Android/Linux ABI numbers even when ART is
hosted by Darwin. Publishing Darwin constants would break APK assumptions and
would also make the same Java value mean different things across devices.
Translation therefore occurs only at the native syscall boundary.

The pinned Android 16 source owns 568 integer fields and one
`initConstants(): void` registrar entry. The gate derives the ordered field and
expression manifest from that source, then verifies every checked-in value by
cross-preprocessing Android 16 Bionic arm64 headers. Only three small Gitless
header archives are fetched (about 1 MB compressed); their normalized path and
file-content trees are pinned because Gitiles gzip bytes are not deterministic.
An exact NDK r28c generated kernel header fills the one file absent from
Bionic's source-generated UAPI.

The Darwin module provides three translation families:

- Linux arm64 open flags to Darwin flags, including the arm64-specific
  `O_DIRECTORY=16384`, `O_DIRECT=65536`, and `O_LARGEFILE=131072` values.
- all 90 Java-visible Android `_SC_*` selectors to same-named Darwin selectors
  where Darwin provides them; unsupported selectors fail capability lookup.
- all 80 Android errno names from same-named Darwin errno values. This handles
  real numeric divergences such as Darwin `ENOTSUP=45` to Android `ENOTSUP=95`
  and Darwin `EAGAIN=35` to Android `EAGAIN=11`.

Run:

```sh
tools/build-android16-os-constants-darwin.sh
```

The managed acceptance reflects over and compares all 568 initialized fields,
creates a file using Java-visible Linux open flags, tests its Darwin `stat` mode
with Android `S_ISDIR/S_ISREG`, translates a real missing-file errno and the
divergent ENOTSUP value, and calls Darwin `sysconf` using Android's
`_SC_NPROCESSORS_CONF=96` selector.

Integration must atomically replace the existing one-field OsConstants owner.
The existing Darwin Linux backend must consume these translation APIs rather
than comparing Java values with Darwin libc constants directly. In particular,
its older asm-generic open constants swap the Linux arm64 meanings of
`O_DIRECTORY` and `O_DIRECT` and must not remain authoritative.
