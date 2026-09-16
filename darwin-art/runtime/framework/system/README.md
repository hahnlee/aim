# System service publication

`ServiceDirectory` owns the immutable startup service table. Its current wire
surface is the legacy `getService`/`checkService` pair retained by
[Android 16 IServiceManager.aidl](https://android.googlesource.com/platform/frameworks/native/+/android-16.0.0_r1/libs/binder/aidl/android/os/IServiceManager.aidl).
Missing names return null; unsupported transactions are not acknowledged as
successful. This is not a complete port of AOSP servicemanager: metadata lookup,
dynamic registration, notifications, access policy and lazy services remain.

`SystemServices` is the Parcel client; native Binder owns channel/FD transport.
The persistent process publishes the directory as its root Binder, and publishes
the existing host package record endpoint under `darwin.package_registry`.
That endpoint is not AOSP PackageManagerService. The directory currently also
publishes the runtime PM and ActivityManager endpoints; their subsystem owners
reside under `runtime/framework/pm` and `runtime/framework/am`.

`process_entry.cc` owns the system-process entry, with an explicit endpoint path
and installed-record resolver supplied by bootstrap. It obtains the canonical
process loader, prepares the framework Looper/shared memory, registers service
endpoints and enters Binder service transport. Production system startup branches
here before fixture class discovery: ProbeActivity/View/Context/PackageManager
and window fixture checks are no longer prerequisites. Local JNI references are
scoped to this entry and failures are propagated without clearing exceptions.

This is a component cutover, not AOSP SystemServer completion. The Java
DarwinSystemServer directory factory and context-loader publication helper are
still migration debt. Original SystemConfig initialization and trusted system
configuration remain incomplete. The isolated JNI test under
`tools/tests/system-process-entry` exercises the actual native entry without any
Probe classes, using test-only service/transport stand-ins; it does not establish
real ART startup, Binder readiness or application acceptance.

The original `services.jar` producer is
`tools/prepare-android16-system-services.sh IMAGE NEW_SERVICES_JAR`. It verifies
the complete source image against `android16-linker-config.lock`, then the exact
JAR against `android16-system-services.lock`, and publishes without overwriting
an existing destination. Its pinned image's framework.jar matches the active
`bootclasspath.lock`; the cached ps16k-r07 image is different and must not be used
as an interchangeable source. `package-system-root.sh` now requires and verifies
this artifact, then includes it at `system/framework/services.jar` unchanged.
The development image builder and Manager bundle builder prepare the same pinned
artifact through `system-services-artifact.sh`. System-process VM configuration
now uses that image's `system/framework/services.jar` plus the support DEX as its
canonical process classpath; APK processes do not receive the service JAR on
their boot or process classpath. The host inventories the JAR in runtime identity
and requires the exact file grant. The support DEX no longer packages copied
AbstractStatsBase/PackageDexUsage definitions; build checks reject their return.
This wiring does not prove successful original service initialization. Run
`bash tools/tests/system-services.sh` with the pinned image
path to verify extraction and rejection cases without changing any profile.

Client and system process must be deployed together: the previous root Binder
was the package record endpoint itself. Do not mask a stale system process by
falling back to that protocol or silently treating lookup failure as not installed.
Current running processes have not yet been migrated merely by building this code.
