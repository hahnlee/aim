# Android 16 ART `libopenjdkjvm` on Darwin

This gate builds the complete one-translation-unit `libopenjdkjvm` module from
the pinned Android 16 ART `Android.bp` and `OpenjdkJvm.cc`. It audits all 55
public `JVM_*` and `jio_*` definitions against matching source-derived and
compiled-symbol manifests, then locks their shared digest. It does not provide
a one-symbol compatibility shim.

The only source change is a hash-locked `__APPLE__` branch in
`JVM_GetLastErrorString`. Darwin exposes the XSI `int strerror_r(...)` ABI, so
the branch captures `errno`, fills the caller's buffer, guarantees termination,
and returns the actual string length. The upstream glibc/Bionic path and every
other JVM service remain unchanged. An arm64 executable smoke checks a real
`ENOENT` message, returned length, termination, and the zero-size contract.

The resulting archive is:

```
_build/openjdkjvm-darwin/libopenjdkjvm-darwin.a
```

Link it after roots such as `libopenjdk`/the UnixFileSystem archive that import
`JVM_*`, and before the normal ART runtime, ART base, Android libbase, and
nativehelper provider archives. Do not force-load it ahead of consumers: normal
archive extraction selects the module when a JVM service is required, while
the final runtime exported-symbol list and `-dead_strip` remove unused services.
The gate compares all 55 public names against the existing runtime/bootstrap,
ART, nativehelper, and libbase archives and fails on any duplicate provider.
