# Android 35 libc++ exception acceptance fixture

This source-derived gate isolates the exception boundary that the basic libc++
consumer intentionally leaves out.

An ordinary NDK shared-library link makes a cleanup landing pad import
`_Unwind_Resume@LIBC_R` from `libc.so`. AIM's provider namespace is
deliberately closed over the pinned libc++ `@LIBC` universe, so silently routing
that request to Darwin's unwinder would be an ABI violation.

The viable vertical slice links the **same pinned NDK r28c arm64
`libunwind.a`** into the consumer. This removes the external `LIBC_R` request.
Its remaining provider calls are pinned `libc.so@LIBC` plus
`dl_iterate_phdr@LIBC` from `libdl.so`; every one already has an exact Darwin
ART provider. The archive's libunwind readability probe is also the exact
`rt_sigprocmask` form accepted by the existing syscall facade. This remains an
Android unwinder: it discovers mapped ELF program headers through the closed
`dl_iterate_phdr` image registry and does not delegate to dyld or Darwin C++.

The exported no-argument function throws a `std::runtime_error` across a
nontrivial `std::string` cleanup frame and catches it in its caller. A result of
`73` proves phase-one search, phase-two cleanup, `_Unwind_Resume`, destructor
execution exactly once, RTTI matching, and catch lifetime. The audit currently
proves the exact build/ELF/provider closure; execution is enabled only after the
real libc++ graph and its 160 providers are live.
The fixture's two-argument `JNI_OnLoad` runs this check and returns JNI 1.6 only
for `73`, allowing the normal ART native-library path to own load and unload.

Run:

```sh
./tools/android35-libcxx-exception-acceptance/audit.sh
```
