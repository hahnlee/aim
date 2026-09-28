# Android 16 libziparchive_for_incfs Darwin gate

This gate builds the exact static `libziparchive_for_incfs` Android.bp
selection: the five common ziparchive translation units plus
`incfs_support/signal_handling.cpp`. It uses
`ZIPARCHIVE_DISABLE_CALLBACK_API=1` and deliberately does not reuse the older
bootstrap archive, which omits `zip_writer.cpp` and compiles with
`INCFS_SUPPORT_DISABLED=1`.

On Darwin the incremental-filesystem signal implementation is an empty object
because its implementation is guarded by `__BIONIC__`; retaining that member is
still required for module identity. No callback or signal shim is introduced.

Run:

```sh
tools/build-android16-ziparchive-incfs.sh
```

The gate creates a six-member ARM64 archive, force-load audits its unresolved
provider manifest, and executes a writer/reader round trip using the upstream
`ZipWriter`, `OpenArchiveFd`, `FindEntry`, and `ExtractToMemory` APIs.
