# Android 16 OpenJDK NIO mapping gate

This gate builds the pinned Android 16 OpenJDK mapping subsystem as three
production translation units. File/channel logic remains unmodified; the
NativeThread TU receives one hash-locked Darwin host-lifecycle patch:

- `FileChannelImpl.c`: complete five-method registrar, including Darwin's
  `mmap`, `munmap`, `lseek`, and `sendfile` path.
- `FileDispatcherImpl.c`: complete fourteen-method registrar. Android's
  `_ALLBSD_SOURCE` portability branch maps the `*64` APIs and `fdatasync` onto
  their Darwin equivalents at module scope.
- `NativeThread.c`: complete two-method registrar and the BSD `SIGIO`
  interruption contract using `pthread_self` and `pthread_kill`. Since Darwin
  ART is embedded in a host process, the patch saves the previous process-wide
  SIGIO disposition and exports a teardown function that restores it after all
  NIO users have quiesced.

Android's `OnLoad.cpp` registers `IOUtil`, then `SocketChannelImpl`, then
`FileChannelImpl` and `FileDispatcherImpl`. `FileInputStream` and
`FileSystemPreferences` follow before `NativeThread`. `IOUtil` is the direct
provider for `fdval`, `convertReturnVal`, and `convertLongReturnVal`; its exact
upstream source plus `jni_util` are built as a separate support archive so an
integrator can preserve that registration order.

The production mapping archive owns exactly the three target registrar
symbols. It must not be force-loaded together with another `libopenjdk` archive
that contains the same source files. Likewise, the support archive contains
the complete `IOUtil` registrar and conflicts with any other owner of that
module. Nativehelper provides `jniRegisterNativeMethods`; OpenJDK `jni_util`
provides the exception helpers.

The host JDK 17 `FileChannelImpl` and `FileDispatcherImpl` native tables are
newer and signature-incompatible with Android 16. The managed smoke therefore
registers the exact 5- and 14-entry tables against isolated acceptance classes,
while compiling the production registrars unchanged. The acceptance-only
`IOUtil` copy changes Java's FileDescriptor field lookup from Android
`descriptor` to host-JDK `fd`; this copy is never archived. `NativeThread` has
the same two signatures on host JDK 17, so its exact production registrar and
signal-handler initialization are executed directly.

The smoke verifies size through the real dispatcher implementation, a
read-only mapping of a temporary file through `FileChannelImpl_map0`, byte
visibility at the returned address, `munmap`, and safe current-thread signaling.
It installs a distinct prior SIGIO handler and proves that the teardown seam
restores that exact disposition.

Run:

```sh
tools/build-android16-openjdk-nio-mapping.sh
```

## ABI boundary follow-up (2026-09-07)

The production archive now separates `FileDispatcherImpl`'s host-constant
`preClose0` `/dev/null` open from `UnixNativeDispatcher`'s Android-flag open;
Android flags never reach Darwin `open(2)`. Bionic errno values are translated
back to Darwin values before OpenJDK's `EINTR`/`EAGAIN` comparisons. Virtual
descriptor `readv`/`writev` use one facade vector operation (with the central
broker's vector entrypoint), and managed acceptance links the redirected
production archive and exercises a virtual-FD `readv` call. The focused NIO,
UnixNativeDispatcher, UnixFileSystem, and Bionic filesystem tests pass.
