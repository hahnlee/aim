# Small vendor HALs: power, sensors and GNSS (ADR 0012, P5)

These HALs report what a Mac can tell Android about its power,
surroundings and position. Each of ours is a Rust AIDL service under `hal/`
that reaches its host side through host-call ([host-call.md](host-call.md)),
built by `cargo aim` ([build.md](build.md)) and placed by `image/overlay.toml`,
like the health HAL. Everything a HAL reports comes from a public or
privilege-free macOS source. What the Mac cannot report is absent, never
faked.

| HAL | Instance | Service | Host module |
| --- | --- | --- | --- |
| power | `IPower/default` V6 | the original (vendor APEX `com.android.hardware.power`) | none |
| thermal | none | none: `thermalservice` is native and reads the Mac ([Thermal](#thermal-no-hal)) | none |
| sensors | `ISensors/default` V3 | `android.hardware.sensors-service.aim` (`hal/sensors`) | 5 `sensors` (`crates/aim-host-sensors`) |
| GNSS | `IGnss/default` V2 | `android.hardware.gnss-service.aim` (`hal/gnss`) | 6 `location` (`crates/aim-host-location`) |

## Power: the original is kept

The image's vendor APEX `com.android.hardware.power` runs AOSP's example
`IPower` V6 (`power/aidl/default`). It accepts every hint, boost and mode
and logs them, and it serves hint sessions. It does not touch the kernel,
so it runs unmodified on the syscall layer. We keep it rather than write
our own, because the Mac offers nothing a power HAL could drive:

- modes flow from Android to the HAL. macOS lets an unprivileged process
  neither enter Low Power Mode (`pmset` needs root) nor change CPU
  frequency or scheduling policy for the machine. The Darwin scheduler
  already places the guest's threads by QoS;
- the reverse direction (the Mac is in Low Power Mode, so Android should
  save power) is not a power HAL input. It would be battery-saver policy
  in the framework. `NSProcessInfo.isLowPowerModeEnabled` is there if it is
  ever wanted.

A HAL of ours would therefore only log, which is what the example does.

## Thermal: no HAL

The image declares no `IThermal`. Its one client was SystemServer's
ThermalManagerService, and `thermalservice` is native now
([system-services.md](system-services.md), "The thermal service"): it
reads macOS's thermal state (`NSProcessInfo.thermalState`) and the HID
event system's temperature sensors (usage page 0xff00, usage 5) in the
service host, through `crates/aim-host-sensors`, as our HAL did through
host-call. With no client left, the HAL went (#624), as a vendor ships no
HAL nothing reads. The vendor APEX's example `com.android.hardware.thermal`,
which reports no temperatures and never throttles, stays removed. The
framework compatibility matrix lists the thermal HAL as optional.

Host-call module 4 (`thermal`) still serves the same reading; no guest
program calls it (#641).

## Private API (optional)

The temperatures (thermalservice) and the ambient light sensor (sensors) come from
IOKit's `IOHIDEventSystemClient*` and `IOHIDServiceClient*` functions
(`crates/aim-host-sensors/src/hid.rs`). IOKit exports them, but they are
**private API**: undocumented, with no compatibility promise. They need no
privileges, and third-party temperature monitors use them.

- They are optional. They are resolved with `dlsym` at run time, never
  linked, so the host binary loads on a macOS without them. When a symbol
  is missing, or a call returns nothing, the host module reports those
  readings as absent: thermalservice omits `CPU` and `BATTERY`, and sensors does
  not list the light sensor. No path depends on them. The thermal state
  (`NSProcessInfo`) and the lid angle (`IOHIDManager`) are public API.
- **Distribution:** App Store review rejects private API use, and a
  sandboxed build may not reach the HID event system. Such a build would
  drop `hid.rs`, which leaves thermalservice with `SKIN` alone and sensors with the
  lid angle alone. No other code changes.

## Sensors

A Mac has few sensors reachable without privileges, so the list is short
and honest:

| Sensor | Type | Source |
| --- | --- | --- |
| Ambient light (handle 1) | `LIGHT`, lux | the `als` HID event service (usage page 0xff00, usage 4), through the optional [private API](#private-api-optional) |
| Lid angle (handle 2) | `HINGE_ANGLE`, degrees | the lid sensor, a HID device (Apple 0x8104, usage page Sensor, usage Orientation) whose feature report 1 holds the angle; public `IOHIDManager`, no permission |

- A sensor is listed only when the host reads it at start-up. A Mac
  without either one (a Mac mini, say) has an empty list, which is valid,
  and apps handle it. There is no accelerometer, gyroscope or magnetometer:
  the Mac's motion sensors are not reachable from user space, and a fake
  one would mislead apps.
- The features follow the Mac the way a vendor declares per-SKU hardware.
  `handheld_core_hardware.xml` declares no `android.hardware.sensor.*`
  feature, since neither sensor is on every Mac. At boot guest-init reads
  the sensors as the HAL does and reports the SKU
  (`androidboot.product.vendor.sku`, `crates/aim-guest-init/src/sku.rs`):
  `light`, `hinge` or `light_hinge`, or none on a Mac with neither.
  SystemConfig then adds the features in
  `/vendor/etc/permissions/sku_<sku>/`: `android.hardware.sensor.light`
  and `android.hardware.sensor.hinge_angle`.
- Both are on-change and not wake-up sensors. While one is active, a poller
  reads the host at the requested rate (between 200 ms and 1 s) and writes
  an event when a value changes, and once on activation. `flush` writes
  `META_DATA_FLUSH_COMPLETE`.
- Events go through sensorservice's fast message queue. `hal/sensors/src/fmq.rs`
  is its writing end: it maps the descriptor's grantors (read and write
  counters, ring, event flag), appends whole events and publishes them
  with a release store of the write counter, as libfmq does, then sets
  `READ_AND_PROCESS` in the event flag and wakes it with
  `FUTEX_WAKE_BITSET`. `Event`'s layout (96 bytes) is the AIDL
  `@FixedSize` layout, which the generated code asserts. The descriptor's
  `quantum` is checked against it.
- The wake-lock queue is unused, since there is no wake-up sensor. Direct
  channels, data injection and `injectSensorData` are
  `UNSUPPORTED_OPERATION`.
- The emulator's multihal (goldfish sensors fed by QEMU) is removed in the
  overlay.

## GNSS

The Mac has no GNSS receiver. The HAL is a location source that reports
CoreLocation fixes as `GnssLocation`s. CoreLocation derives them from
Wi-Fi positioning or a nearby iPhone's GNSS.

- The host module owns one thread with a run loop, since CoreLocation
  delivers updates on the run loop of the thread that created its
  `CLLocationManager`. `FN_START` and `FN_STOP` start and stop updates.
  `FN_READ` copies the manager's latest `location` (no delegate is needed)
  with the current authorization.
- The HAL polls at the `setPositionMode` interval (at least 1 s) and
  delivers each new fix once. `RECURRENCE_SINGLE` stops after the first
  fix.
- Session callbacks (status and location) come from the HAL's own
  reporting thread, in order, after the call that caused them has returned
  and with no HAL lock held, as a receiver's chip reports asynchronously.
  The framework calls `start` and `stop` holding locks its callbacks take,
  so a status callback inside `stop` deadlocked system_server (#459).
  Capabilities and system info are answered within `setCallback`, as
  AOSP's default HAL does.
- A fix carries CoreLocation's accuracies: horizontal accuracy always;
  altitude, speed and bearing, each with its accuracy, only when
  CoreLocation marks them valid (non-negative). `elapsedRealtime` is
  `CLOCK_BOOTTIME` now, minus the fix's age.
- Capabilities: `SCHEDULING` only. There are no satellites, NMEA,
  measurements or navigation messages. The nullable extensions are null,
  the others `UNSUPPORTED_OPERATION`, which the framework treats as
  absent. `yearOfHw` is 0 (unknown).
- Authorization: the first start asks for When-In-Use access, so macOS shows
  its Location Services prompt once. Nothing changes system settings. With
  Location Services off, or access denied, the HAL logs the reason once and
  reports no fix. Android sees a location provider that has not yet
  fixed.
- V2 is the emulator's version and is accepted by the device's target FCM
  level (8, `2-3`) and by later matrices.

## Verified (2026-09-28, MacBook Pro M2 Pro, macOS 27, lid closed)

- Host crates: `cargo test -p aim-host-sensors -p aim-host-location`.
  They check the argument blocks and read plausible values, for example
  CPU 51.9 °C, battery 36.7 °C, state nominal, light 0 lux and lid 0°
  with the lid closed. Location reports `SERVICES_OFF`.
- HAL unit tests are built for `aarch64-linux-android` and run under
  `linux-run`: the FMQ writer (wrap-around, no overwrite, the event flag),
  the sensor list, the thermal mapping and the fix conversion.
- End to end on the derived image, with `guest-init --run --only
  logd,servicemanager,vendor.power-default,vendor.thermal-aim,vendor.sensors-aim,vendor.gnss-aim`:
  - the original `service list` shows all four instances;
  - `IThermal` `getTemperatures` (`service call ... 3`) returns
    `skin` NaN/`NONE`, `cpu` 60.9 and `battery` 37.1;
  - `ISensors` `getSensorsList` (5) lists the light and lid sensors;
  - `hal/sensors/examples/sensors_client.rs` plays sensorservice. It
    builds a memfd queue, calls `initialize`, activates both sensors and
    flushes, waits on the event flag with a cross-process bitset futex, and
    reads a flush-complete, `LIGHT 0` and `HINGE_ANGLE 0`;
  - `IGnss` `start` (14) succeeds, and the HAL logs that Location Services
    are off. `getExtensionGnssConfiguration` (4) is `UNSUPPORTED_OPERATION`;
  - the original power HAL answers `isModeSupported(LOW_POWER)` true,
    takes `setMode(SUSTAINED_PERFORMANCE, true)`, and returns a preferred
    hint-session rate of 1 ms.

## Gaps

- `/dev/ashmem` is not emulated. libcutils uses it for
  `ashmem_create_region` while `sys.use_memfd` is false (the default), so:
  - the power HAL's `getSessionChannel` fails, and hint sessions fall back
    to binder calls;
  - sensorservice cannot yet create its event queue, so the sensors HAL is
    exercised end to end only by the example client.
- The syscall layer's shared futexes have no bitsets (Darwin's shared
  `__ulock` has none). A bitset wait is treated as a plain wait, which
  gives spurious wakeups that libfmq tolerates.
- Before `apex.all.ready`, libvintf reads the vendor APEXes' VINTF
  fragments from `/bootstrap-apex`, which lists only the `vendorBootstrap`
  ones (the gatekeeper HAL); the others' `addService` is refused until
  then. guest-init's apexd role sets the property with
  `apexd.status=ready`.
