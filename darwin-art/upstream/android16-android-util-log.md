# Android 16 android.util.Log Darwin registrar

`android.util.Log` is owned by `android_util_Log.cpp` inside the host-supported
`libandroid_runtime` Soong module; it is not a standalone Soong library. The TU
atomically owns this complete three-entry table:

- `isLoggable(String, int): boolean`
- `println_native(int, int, String, String): int`
- `logger_entry_max_payload_native(): int`

The first two existing Darwin compatibility registrations are therefore not a
complete Android 16 owner. The payload native must not be added as a fourth
ad-hoc entry: integration should replace the old partial table with
`register_android_util_Log` from this archive.

For the host variant, `HostRuntime.cpp` exposes Log in `gRegJNIMap`; actual
execution order follows the `core_native_classes` CSV rather than unordered-map
iteration. The device `AndroidRuntime.cpp` has the fixed sequence SystemClock,
CharsetUtils, EventLog, Log, then MemoryIntArray. The independent gate verifies
the host property-driven dispatch contract and the device table ordering.

DarwinART invokes the complete registrar from the process-wide framework
registration phase, before Binder can initialize StrictMode. It is not owned
by the later AssetManager/resource installation phase.

The exact TU's Darwin link dependency is the pinned AOSP host `liblog` archive,
which provides `__android_log_assert`, `__android_log_buf_write`,
`__android_log_is_loggable`, and `__android_log_print`. Its libutils, libbase,
libsystem, and nativehelper inputs are header closure only for this TU; the
object has no corresponding provider imports. Homebrew or macOS logging
substitutes are not used.

Run:

```sh
tools/build-android16-android-util-log.sh
```

The gate sparsely materializes only the exact upstream TU, its direct headers,
the managed class, and both registration-order sources. It emits the one-member
Darwin arm64 registrar archive at
`_build/android-util-log/libandroid-util-log-registrar-darwin.a`. A managed
OpenJDK acceptance loads the exact registrar and invokes all three methods,
including the locked 4,068-byte payload value, a real loggability query, a real
liblog write, and the upstream null-message exception behavior.
