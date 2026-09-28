# Android 16 `java.io.FileDescriptor` Darwin owner

This gate builds the complete Android 16 OpenJDK `FileDescriptor_md.c` owner
from pinned libcore revision `080fac8bb8670bc7fbc895050caf4b13c4d6cd12`.
The single registrar owns all three managed natives: `sync()V`, CriticalNative
`isSocket(I)Z`, and CriticalNative `getAppend(I)Z`.

The hash-locked Darwin patch preserves the upstream BSD branches while routing
descriptor extraction through Android's device-layout nativehelper API. The
archive therefore targets the Android `descriptor:I` field and must not be
combined with the host-layout `fd:I` nativehelper variant. Managed acceptance
executes fsync, socket detection, and append-flag detection against real Darwin
descriptors.

Integration atomically replaces the earlier two-method compatibility table and
calls `register_java_io_FileDescriptor` before IOUtil or FileInputStream can
initialize the class. Link the consumer first, then this archive,
FileInputStream support, `libopenjdkjvm` for `JVM_Sync`, device nativehelper,
and liblog. Partial or repeated registration is forbidden.

Run:

```sh
bash tools/build-android16-file-descriptor-darwin.sh
```
