# Android 16 `java.io.FileInputStream` Darwin owner

This gate builds the complete Android 16 OpenJDK `FileInputStream.c` native
owner from pinned libcore revision
`080fac8bb8670bc7fbc895050caf4b13c4d6cd12`. Its exact registrar is
`register_java_io_FileInputStream`, and its complete table is:

```
length0    ()J
position0  ()J
skip0      (J)J
available0 ()I
```

Android intentionally removed `open0` from this table in favor of IoBridge;
single-byte/buffer reads and close are also owned by the common stream and
FileDescriptor paths. The managed acceptance therefore opens and closes a real
temporary file through those paths while replacing all four methods above. It
checks single and buffered reads, length, position, initial and post-skip
availability, skipping, close, and the closed-stream exception.

The archive contains the four upstream support TUs required by this owner:
`FileInputStream.c`, `io_util_md.c`, `jni_util.c`, and `jni_util_md.c`. The sole
hash-locked Darwin portability patch changes `io_util_md.c::getFD` to call the
real nativehelper `AFileDescriptor_getFd` API. Consequently the same compiled
owner uses Android's `descriptor:I` with the device-layout nativehelper, while
the independent host-JVM acceptance uses its `fd:I` nativehelper variant. No
field-ID shim or per-method implementation is present.

The result is:

```
_build/file-input-stream-darwin/libopenjdk-file-input-stream-darwin.a
```

Link order is consumer/registrar reference, this archive, the genuine
`libopenjdkjvm-darwin.a`, then the normal ART runtime/core providers,
device-layout nativehelper, full Android libbase/liblog, and system frameworks.
The final runtime must retain its exported-symbol policy and `-dead_strip` so
unrelated `io_util_md.c` FileDescriptor functions do not retain `IO_fd_fdID`.
The gate proves a device-provider closure with no unresolved
`AFileDescriptor_getFd`, `jniRegisterNativeMethods`, `JVM_GetLastErrorString`,
or `IO_fd_fdID`.

Any existing `available0`-only or other partial FileInputStream registration
must be removed atomically before invoking this registrar. A later
`RegisterNatives` call would silently replace overlapping slots and split
ownership. The gate also scans current runtime/bootstrap/libcore archives for
duplicate global registrar or method definitions and fails if it finds one.
