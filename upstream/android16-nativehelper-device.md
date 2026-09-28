# Android ART nativehelper provider on Darwin

Android 16 has two source-complete nativehelper variants that are ABI-compatible
at the C symbol level but expect different managed `java.io.FileDescriptor`
layouts:

- `platform/libnativehelper` is `host_supported` and is the correct provider
  for Android classes running on ART. It reads `descriptor:I` and writes through
  `FileDescriptor.setInt$(I)V`.
- `frameworks/base/libs/nativehelper_jvm` is layoutlib's host-JDK adapter. It
  reads and writes the host field `fd:I` directly.

The prebuilt Android 16 `core-oj.jar` used by this project has the Android
`descriptor:I` layout. Using layoutlib's provider makes the first lazy
JniConstants initialization abort with `Field not found: fd:I`.

`tools/build-android16-nativehelper-device-foundation.sh` builds all seven TUs
from the checksum-locked `platform/libnativehelper` Android.bp closure:

```text
libnativehelper_any_vm:
  DlHelp.c
  ExpandableString.c
  JNIHelp.c
  JniInvocation.c

libnativehelper:
  JNIPlatformHelp.c
  JniConstants.c
  file_descriptor_jni.c
```

It emits:

- `_build/nativehelper-device-foundation/libnativehelper-device-darwin.a`
- `_build/nativehelper-device-foundation/device-sources.txt`
- `_build/nativehelper-device-foundation/device-definitions.txt`
- `_build/nativehelper-device-foundation/host-definitions.txt`
- `_build/nativehelper-device-foundation/file-descriptor-layout.txt`
- `_build/nativehelper-device-foundation/layout-smoke.log`

The smoke test drives the genuine `AFileDescriptor_getFd` and
`AFileDescriptor_setFd` implementations through a JNI-table harness. It proves
that lazy initialization requests `descriptor:I` and `setInt$`, never `fd:I`.
No source field is renamed or patched.

The existing `_build/nativehelper-foundation/libnativehelper_jvm.a` remains the
layoutlib/host-JDK provider. Resource JNI and the Darwin AndroidRuntime closure
consume the new ART/device archive instead.

## Graphics closure boundary

The existing precomposed graphics closure also contains layoutlib's host
JniConstants definitions, including `_JniConstants_FileDescriptor_fd`. Merely
adding the device archive after that object is not an atomic fix: it can either
create overlapping common JniConstants definitions or leave graphics code
bound to the `fd:I` variant.

Final ART integration therefore needs a separately named graphics closure
assembled with `libnativehelper-device-darwin.a` in the nativehelper provider
slot. The original host-layout graphics closure must remain available for
layoutlib testing. This gate deliberately does not rewrite that parent-owned
runtime/link integration.
