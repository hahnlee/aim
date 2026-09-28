# Original SystemProperties JNI import

Run `bash tools/build-android16-system-properties.sh` after the existing
nativehelper/libbase/libutils source prerequisites have been prepared.
The pinned original JNI source is compiled without source patches. A forced
include selects ART's CriticalNative ABI; macOS is not layoutlib. Defining
`__ANDROID__` globally instead selects unrelated bionic pthread APIs in host
headers and is not the appropriate boundary.

The standalone test includes the original source to type-check its private
CriticalNative getters and executes their parsing against original libbase's
host property store. It does not test actual ART dispatch, Rust property
storage, Binder authorization or cross-process properties. Unused JNI functions
are dead-stripped, not replaced by success stubs.

The production registrar delegates to the original JNI registrar, and both
runtime link paths include this archive. `production_jni_abi.h` binds reads to
the installed process-state property area and writes to the original AOSP
property-service client. The build rejects unprefixed host property imports.
The standalone host-store test is not part of the production archive.

The setter boundary clears guest errno and translates a failed call's errno
through the errno owner's name-derived mapping for the host JNI diagnostic.
The original return value is preserved. A missing service remains a failure;
this integration does not supply a service endpoint, authorization, successful
mutation, or cross-process change notification. Unknown guest errno values
cannot be formatted by host strerror and use the original generic failure
diagnostic, rather than displaying an unrelated Darwin error.
Libutils owns the original callback fanout; setter completion does not invent
an independent callback. Other libbase property consumers still require an
audit before claiming all property APIs share this authority.
Original JNI keeps its VM/class callback reference for process lifetime;
the runtime's restart/drain contract must account for that ownership.
