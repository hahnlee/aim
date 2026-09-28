# Immutable Bionic process-state facade

This standalone gate supplies `getenv`, `__system_property_get`, and `getauxval`
from one explicitly constructed immutable Android snapshot. It never reads the
Darwin environment, Darwin process auxiliary state, `/Users`, or a host property
service, and has no dynamic-loader fallback.

Environment and property bytes are validated, NUL-terminated, and stored in
stable Box allocations. `getenv` pointers and the 16-byte `AT_RANDOM` pointer
remain valid until facade teardown. Unknown environment names return null and
leave Bionic errno unchanged. Unknown properties return length zero, write an
empty value, and leave errno unchanged. Property values are limited to 91 bytes
plus NUL (`PROP_VALUE_MAX == 92`).

The auxv allowlist is `AT_PAGESZ`, `AT_HWCAP`, `AT_HWCAP2`, `AT_SECURE`, and
`AT_RANDOM`. HWCAP is conservatively restricted to the AArch64 FP/ASIMD
baseline and HWCAP2 to zero; no Darwin feature bit is reinterpreted. Unknown
keys return zero with Android `ENOENT`, including when allowlisted zero values
would otherwise be ambiguous.

Activation holds a process-global `Arc`; lookups briefly clone it and then read
immutable maps without mutation. Teardown is only valid after all guest threads
and borrowed pointers are quiescent. It removes the global owner and frees all
backing allocations exactly once. Calls after teardown fail closed with Android
`EIO` and a sticky capability marker rather than consulting host globals.

`audit.sh` pins NDK r28c headers, actual libc++ import classifications, and the
Bionic errno provider. A real Android AArch64 ELF verifies absence of host HOME,
property boundary semantics, auxv zero/unknown distinction, pointer stability,
8 pthreads performing 1,000 guest lookup rounds each, and post-teardown failure.
