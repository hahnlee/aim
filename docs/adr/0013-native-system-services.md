# ADR 0013: Android's system services as native macOS implementations

Status: accepted 2026-09-29 (migration tracked in #153)

## Context

ADR 0012 runs the pinned Android userspace unmodified on a Linux syscall
layer. It works: the original SystemServer, SurfaceFlinger and apps run, and
compatibility comes from the original code. It also means every start boots
a whole Android system before the first app window:

- a boot runs ~70 services and ~25 one-shot programs, then SystemServer
  starts over a hundred Java services, mostly in order, then SystemUI, the
  launcher, the keyguard and the wallpaper; first boot completes after
  about 40 s on an M2 Pro;
- most of that serves an Android device shell (status bar, home screen,
  lock screen, recents) that a Mac already provides, and that window mode
  (#294) hides;
- snapshots or a resident guest can hide the cost but not remove it.

Wine is fast because it does not run Windows: it implements the Windows API
on the host and runs each program as one host process. The same is possible
for Android: an app's own process (ART, framework.jar, its native
libraries) talks to the system almost only through binder services
(ActivityManager, PackageManager, WindowManager, notification, clipboard,
input method, display, power, location, ...). If those services are native
implementations backed by macOS, no Android system has to boot.

The first stack (ADR 0012's context) tried a version of this and failed:
hand-written endpoints that mimicked AOSP owners by reflection, rebuilt and
patched ART and frameworks, by-name special cases in the loader. What
failed was how it was done: unversioned reflection, no conformance bar,
and everything replaced at once. Replacing services is still the right idea.

## Decision

Move Android's system services, one at a time, from the original
SystemServer to native implementations backed by macOS, until no Android
system has to boot. Keep an original component only where compatibility
truly requires it. The runtime stays working at every step: this is a
migration from the ADR 0012 stack, not a rewrite.

1. **What stays original.** The app's own process is untouched: ART and the
   boot image, framework.jar and the other boot classpath jars, bionic, the
   linker, the app's native libraries, all on the ADR 0012 syscall layer
   with the binder driver and the HALs. So is anything whose exact
   semantics apps depend on and that has no macOS owner, until a
   replacement passes its conformance bar (below).
2. **The replacement unit is a binder service.** A replacement implements
   the service's AIDL interface at the pinned image's version, with
   transaction codes generated from the pinned AIDL (never reflection), and
   is registered with servicemanager under the original name. The original
   service is then not started. Apps and the original framework code in
   their processes cannot tell the difference.
3. **Where a replacement lives.** It is Rust and runs on the host (in the
   binder host or its own process), using macOS for what macOS owns:
   NSWindow for windows and focus, the notification center, NSPasteboard,
   the Mac's input methods, displays, power and battery, location,
   networking, locale and appearance. It keeps whatever state Android
   persists (settings, package state) in Android's formats while the
   original still reads them.
4. **Conformance bar.** A service is replaced only when its replacement
   passes the CTS module(s) for that service's API, and the app checks
   (the integration gate, Settings, Chrome, Calculator and the tracked
   game set) still pass. Failing tests keep the original. The test results
   are recorded with the change.
5. **Order.** Largest cost and clearest macOS owner first:
   1. **The shell**: launcher, SystemUI (status and navigation bars,
      notification shade, quick settings, keyguard, recents, volume UI,
      toasts, and the window-management shell WMShell that window mode
      uses), wallpaper, boot completion without a home activity. macOS
      provides the shell; the task bridge (`aim-windows`) takes over task
      organization.
   2. **Services with a macOS owner**: notification (display and actions,
      #4), clipboard, input method (#23), display, power and battery,
      location, connectivity state, locale, time zone and appearance
      (#281-#283), audio policy, vibrator.
   3. **Services that start on demand**: SystemServer services that no
      replacement covers yet start when first requested instead of at boot
      (a SystemServer exception under ADR 0012's rules), until they are
      replaced.
   4. **Core**: PackageManager, ActivityManager/ActivityTaskManager,
      WindowManager. Last, and each only after its conformance bar.
   When the core is replaced, SystemServer no longer boots.
6. **What an app start costs at the end.** A resident host process (the
   binder host with the native services, like wineserver) and, per app,
   one process from a preloaded zygote snapshot: the target is a Wine-like
   start, about 1-2 s from a click to a window.

## Consequences

- ADR 0012's "unmodified, only below the guest" principle now applies to
  the app's process and to originals not yet replaced. System services move
  above the line: we own them.
- Android upgrades cost more than a new image pin: every replaced service
  tracks its AIDL and CTS at the new version. The generated transaction
  codes and the CTS bar make that work mechanical rather than guesswork.
- Google Play services and apps that inspect the system (device policy,
  accessibility, launchers) depend on many services at once. They are the
  compatibility risk and are tested at every step.
- The launcher and SystemUI stop running in window mode. They stay for the
  device-window mode until the shell is replaced there too.
- AGENTS.md changes with this ADR: replacing a system service at its binder
  interface, gated by CTS, is no longer an exception. Reflection, name
  interception and app-specific branches stay forbidden.

## Measurements

Each migration step records targeted measurements of what it changes (the
binder trace of the calls it serves, their latency before and after, a
smoke boot with a cold app start) and its CTS results, in
[system-services.md](../system-services.md) and the step's PR; no full
benchmark runs.

## Steps

The migration's state and conformance results are in
[system-services.md](../system-services.md).

### M1: the replacement pipeline, on clipboard (2026-09-29)

- **Generated AIDL.** The framework interfaces a native service serves or
  calls are generated from the pinned `.aidl` files
  (`crates/aim-services/sources.lock`) into `aim-service-aidl`; every
  transaction code is checked against the stubs in the image's
  `framework.jar`.
- **The service host.** Native services are a binder process of the
  driver inside guest-init (`aim_binder_host::local`), registered with
  servicemanager when it is ready.
- **The SystemServer exception.** SystemServer has no configuration to
  leave a service out, and rebuilding `services.jar` from source would
  take the platform build. The derived image's `services.jar` is the
  original with the one `startService(Foo.class)` call of each service in
  `image/native-services` turned into `nop`s in place: a verified,
  symbolic edit (the class, the call and its unused result are checked,
  so a changed SystemServer fails the build instead of being patched
  wrongly), recorded as a `replace` in `image/overlay.toml`. The jar's
  oat files name its entries by CRC; the `oat` node compiles them for the
  edited jar, as it compiles the image's other oat files with code (ADR
  0012, decision 4), with the original's `speed-profile` and its profile
  converted to the edited jar's checksums. Without them system_server
  verified services.jar at run time and a cold Settings start after boot
  timed out. It is the
  ADR 0012 exception for SystemServer that decision 5.3 anticipated.
- **The pilot.** `clipboard` on `NSPasteboard` passes 35 of CTS's 36
  clipboard tests (the original: 36); the missing one needs DeviceConfig
  (#428). What system_server-internal state native services need, and how
  their permission checks are made fast, are #430 and #432.
