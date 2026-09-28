# Android 16 `java.io.UnixFileSystem` Darwin owner

This gate builds the complete 12-entry Android 16 OpenJDK
`java.io.UnixFileSystem` JNI table from `platform/libcore` at
`080fac8bb8670bc7fbc895050caf4b13c4d6cd12`. The archive contains the pinned
upstream registrar implementation and its complete Unix-filesystem
canonicalization/JNU support closure; it does not copy or reimplement
individual native methods. `io_util_md.c` is included whole because it owns the
upstream `handleOpen` implementation used by exclusive creation. Its unrelated
FileDescriptor entry points remain separate function/data sections so a final
Darwin link can dead-strip them rather than inventing field-ID providers.
The sparse materialization also pins every private header used by that closure,
including `jlong.h`, `jlong_md.h`, and `classfile_constants.h`, before any
source is compiled.

The upstream source already contains its macOS branches for directory entry
names, timestamp fields, `statfs`, and CoreFoundation-backed platform strings.
The gate compiles those branches with `MACOSX` and `_ALLBSD_SOURCE`, verifies
all 12 exact managed descriptors, and runs every operation against a temporary
directory. The managed acceptance includes `list0`, canonicalization, file and
directory attributes, exclusive creation, permission changes, timestamps,
name limits, and filesystem space.

Integration must replace the existing two-entry Darwin table atomically with
`register_java_io_UnixFileSystem`. Registering either partial owner before or
after this registrar is forbidden: `RegisterNatives` would silently replace
overlapping method slots while leaving ownership split across implementations.

Link the archive before the device-layout `libnativehelper` provider and retain
the runtime dylib's exported-symbol list plus `-dead_strip`. This keeps the
needed upstream `handleOpen` section while removing `io_util_md.c`'s unrelated
FileDescriptor entry points and their `IO_fd_fdID` state. The retained external
VM contract is `JVM_GetLastErrorString`, owned by ART's genuine
`libopenjdkjvm`; the gate records it in
`managed-retained-undefined.txt` rather than supplying a per-symbol shim.

## ABI and private-root follow-up (2026-09-07)

The filesystem owner now keeps Android open flags on the Bionic side and only
translates them at the Darwin syscall boundary. Private-root host opens walk
from an owned root descriptor with `openat` and `O_NOFOLLOW` on every component,
covering both creates and read-only opens without a canonicalize-then-open
TOCTOU window. The focused managed UnixFileSystem gate and Bionic filesystem
unit suite pass, including final-symlink rejection.
