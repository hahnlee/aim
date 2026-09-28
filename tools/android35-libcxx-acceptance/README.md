# Android 35 libc++ acceptance fixture

This gate pins the NDK r28c/API 35 arm64 `libc++_shared.so` and proves two
separate boundaries without substituting a Darwin C++ runtime.

`audit.sh` first gives the real 9 MiB ELF to `aim-elf-loader`. A closed
placeholder resolver is used only for a **non-executing structural proof**: all
4,267 supported RELA entries are applied, all 160 strong `@LIBC` imports plus
the one optional unversioned `__cxa_thread_atexit_impl` are observed, GNU
versions and `DT_NEEDED` ownership are checked, RELRO is sealed, and a real
libc++ export is looked up. No initializer or finalizer is called with the
placeholder addresses.

The gate then builds `libaim_libcxx_consumer.so`. Its direct
dependencies are the pinned `libc++_shared.so` and Android's public `libc.so`
stub. Linking the latter is deliberate: compiler-emitted calls such as
`memcpy` retain their `LIBC` GNU-version requirement instead of becoming an
unversioned hole in the closed namespace. The no-argument
`aim_libcxx_collections` function crosses libc++ heap and ABI boundaries
with a non-SSO `std::string`, `std::vector`, `std::sort`, bounds checking,
allocation, and deletion. It also calls `std::filesystem::copy_file` to copy
`/libc++_shared.so` from the immutable graph root into private
`/data/libcxx-copy.so`, then compares both `file_size` results. A real
closed-namespace execution must return `189` after libc++, libc, libm, and
libdl providers have been initialized.
Its two-argument `JNI_OnLoad` runs that check internally and returns JNI 1.6
only on success, so ART's ordinary generic native-library path can execute the
acceptance without exposing a raw Android function pointer to Darwin code.

Exceptions remain outside this particular consumer. The adjacent exception
acceptance statically links the pinned Android `libunwind.a`, exercises
cross-frame nontrivial cleanup and catch, and therefore does not invent a
`LIBC_R` host provider. Iostream remains deferred; it adds global stream
initialization and obscures this focused collection/filesystem path.

Run:

```sh
./tools/android35-libcxx-acceptance/audit.sh
```
