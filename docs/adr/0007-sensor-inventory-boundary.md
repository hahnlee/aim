# Android sensor inventory on Darwin

Status: adopted 2026-09-19; physical Chromium acceptance pending.

## Context

An unchanged Chromium navigation opens `Context.SENSOR_SERVICE`. Android 16
`SystemSensorManager` then calls `nativeClassInit`, but this runtime had no
SensorManager JNI registration and the Browser died with
`UnsatisfiedLinkError`. The NDK `ASensor*` facade separately exposed an empty
list while creating an event queue with no backing source.

The pinned AOSP JNI owner depends on `libsensor`'s `SensorManager`, event queue,
Looper integration and native sensor service. These are not materialized in
this runtime. Copying the upstream JNI alone would leave its service and
resource-lifetime contracts unresolved. macOS exposes HID sensor devices on
the host, but no Android-compatible device mapping or event delivery has been
implemented or validated here.

## Decision

Keep the pinned Android `SystemSensorManager.java` as the owner of sensor
enumeration, listener policy, permissions and direct-channel validation.
Register its exact Android 16 JNI methods in a sensor-owned adapter. The Java
JNI and NDK `ASensor*` API share one process-lifetime provider identity and
inventory. Its inventory contains **zero mapped Android sensors**, which is a
statement about current runtime support, not about host hardware.

Enumeration stops without modifying a Java `Sensor` or inventing metadata.
The manager identity is stable because Java has no native destroy method.
Injection and direct channels report unavailable; no synthetic event queue,
handle, callback or sample is produced. Calls requiring a nonexistent queue
fail explicitly. Invalid manager identities are rejected before use.

The implementation resides in `compat/sensor`; framework startup only invokes
its registrar, and the old NDK sensor functions leave the broad platform
translation unit. Future host IOHID mappings and event delivery belong in
this provider, with Android sensor type/permission behavior remaining in the
framework. When the pinned `libsensor`/sensorservice owner becomes operational,
replace the adapter with upstream JNI without changing Java policy.

## Verification

Check the JNI method names/descriptors against the pinned AOSP source; exercise
real `Context.SENSOR_SERVICE` construction and repeated Chromium physical
link/back/reload. Verify Java and NDK inventories agree, invalid operations
fail, both runtime flavors link, and the exact Ninja rebuild is no-work.
