# AOSP property-service client

`bash tools/build-android16-property-client.sh` verifies and compiles unmodified
Android 16 `system_property_set.cpp` with pinned supporting headers. It retains
protocol negotiation, property-service Unix socket requests and acknowledgement
handling. It does not mutate the process-local property read area.

Android layouts are compiled for native Darwin arm64 PCS; external imports are
renamed to prevent accidental host libc resolution. The next integration must
bind socket/errno/logging/formatting to their explicit guest owners and route
the service endpoint to a credential-aware property-service owner. Property
updates and read-area notifications must originate there, not in the caller.

Fixed-argument imports now forward to explicit guest providers in bindings.c.
The build partially links them with the original client and rejects unexpected
remaining imports. Logging reuses the pinned AOSP async-safe formatter with
the reviewed Darwin stderr sink patch, a private symbol prefix, guest errno
and guest strerror. Abort delegates to the existing abort owner. A formatter
test checks native varargs, Android `%m` and preservation of both errno cells.
The bound client is included in the runtime provider closure. Its component
test runs the original setter with no registered service, checks non-success
and verifies that the local read area is unchanged. The default test snapshot
lacks ro.property_service.version, so this currently exercises protocol v1.
The process-state namespace adapter now exports the original setter at LIBC.
Successful service transactions are not verified yet.
The existing JNI property map and process-local native read area remain separate
migration debt. Compiling this client does not fix that split.
