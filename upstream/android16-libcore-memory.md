# Android 16 libcore.io.Memory Darwin gate

Android 16 intentionally splits the `libcore.io.Memory` native table across
two additive owners. ART runtime initialization first registers seven
`peek*Array` methods from `art/runtime/native/libcore_io_Memory.cc`. Later,
libjavacore `JNI_OnLoad` registers the remaining eighteen methods from
`libcore/luni/src/main/native/libcore_io_Memory.cpp`, including `peekByte`,
unaligned scalar access, all `poke*Array` methods, unsafe bulk conversion, and
`memmove`.

This gate builds the exact one-TU libcore owner as a Darwin arm64 archive. It
also builds the exact one-TU upstream `JniConstants.cpp` provider as a separate
archive, keeping provider ownership and duplicate detection explicit. The
only compile-time portability layer provides glibc's `byteswap.h` names using
Clang builtins and redirects the shared `sys/sendfile.h` include to Darwin's
`sys/socket.h`. Memory does not call sendfile. No native method is replaced or
stubbed. Both pinned method manifests are compared with the 25 native methods
declared by Android's managed `Memory.java`.

The managed acceptance registers the production eighteen-entry table against
an isolated class of the exact binary name. It checks byte access, unaligned
short/int/long access, swapped and unswapped primitive arrays, odd short-array
counts, byte-array offsets, and `unsafeBulkGet`/`unsafeBulkPut` byte-order
semantics. `memmove` remains in the complete registrar but is not invoked by
the host-JDK smoke because its direct provider is libjavacore's complete
`JniConstants` cache, whose initialization requires Android framework classes.

Integration order is ART's seven-entry owner followed by this archive's
eighteen-entry owner. They are complementary and must both remain. The archive
must not be force-loaded with another libjavacore archive containing the same
`libcore_io_Memory.cpp`, which would duplicate the global registrar and all
eighteen implementations. Nativehelper supplies header-only JNI registration
and array helpers; direct linked providers are liblog plus the emitted full
libjavacore `JniConstants` archive for `memmove`. If an integration already
links another full `JniConstants.cpp`, it must not force-load the provider
archive again.

Run:

```sh
tools/build-android16-libcore-memory-darwin.sh
```

## ART runtime collision-free variant

The ICU-JNI archive already linked by the runtime defines five global
`JniConstants` methods. Exact symbol auditing finds three collisions with the
full libcore cache: `Initialize`, `Invalidate`, and `GetStringClass`. Because
each cache is one archive member, resolving `GetPrimitiveByteArrayClass` would
extract all 37 libcore definitions and cause duplicate globals.

The gate therefore emits the recommended ART-runtime pair:

- `libcore-memory-art-runtime-darwin.a`
- `libcore-jni-constants-art-runtime-darwin.a`

Both upstream translation units are compiled consistently with the module
class token renamed to `AimLibcoreJniConstants`. All 37 cache methods are
preserved, and the gate links the renamed pair alongside the real ICU-JNI
`JniConstants.o` to prove zero collisions. This is not a per-symbol shim. The
external registrar remains exactly `register_libcore_io_Memory`; only its
private provider edge changes. The unrenamed archives remain useful for a
standalone libjavacore that does not link ICU-JNI, but must not be used in the
current ART runtime dylib.
