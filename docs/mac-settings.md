# The Mac's time zone, language and appearance

The device follows three of the Mac's settings (#283, #282, #281). The
host reads them and the guest applies them through Android's own services,
the way a device's vendor init would; no part of Android is patched, and
the Mac's settings are only read.

| Setting | Mac source | Property | Applied by `init.aim.rc` |
| --- | --- | --- | --- |
| Time zone | the IANA name `/etc/localtime` links to, if the image's tzdata (`/apex/com.android.tzdata/etc/tz/tzdata`) has it | `vendor.aim.mac.time_zone` | at boot: `setprop persist.sys.timezone` in `post-fs-data`; while running: `cmd alarm set-timezone` (`AlarmManager.setTimeZone`) |
| Language and region | the first of `AppleLanguages` Android can name, with `AppleLocale`'s region when it has none (`ko` + `ko_KR` → `ko-KR`, `zh-Hans` + `zh_CN` → `zh-Hans-CN`) | `vendor.aim.mac.locale` | at boot: `setprop persist.sys.locale` in `post-fs-data` |
| Appearance | `AppleInterfaceStyle` (`Dark`, or absent for Light; "Auto" updates it) | `vendor.aim.mac.night_mode` (`yes`/`no`) | `cmd uimode night` (`UiModeManager.setNightMode`, `ui_night_mode`), at `sys.boot_completed` and on each change |

- **Host.** `aim-guest-init` (`src/mac.rs`) sets the properties after
  `PropertyInit` and before init's first action, then reads the Mac's
  settings every 2 s and sets a property again when its value changes.
  CoreFoundation is opened on first use (`aim_hostcall::dylib!`).
- **Boot.** `post-fs-data` in `init.aim.rc` runs after `init.rc`'s, which
  loads the persistent properties, and before `zygote-start`. So
  system_server, bionic and libcore start with the Mac's zone and
  language, whatever was persisted.
- **While Android runs.** A property trigger runs the original `cmd` as
  `system`. `AlarmManager.setTimeZone` persists the zone, broadcasts
  `ACTION_TIMEZONE_CHANGED`, and the time zone detector reads the device
  zone back from it (`dumpsys time_zone_detector`:
  `getDeviceTimeZone()`). The detector's manual suggestion is not usable:
  with automatic detection on (the default, with the emulator's telephony
  and geolocation declarations) its capability is "not applicable".
- **The appearance at boot** is applied at `sys.boot_completed`, so a
  boot after the Mac changed shows the previous appearance until then.
  UiModeManager takes a change from its start, before the system is
  ready, but then never sends the new mode to the global configuration
  (`am get-config` has no `night`/`notnight`).
- **Changes made inside Android** hold until the Mac's setting changes
  again or the device boots: a property trigger fires only on a new
  value. Whether an in-Android change should instead stick, or the Mac
  should always win, is open (#281).
- **The clock.** `CLOCK_REALTIME` is the Mac's; only the zone was wrong.

## Limits

- Only the first language: `persist.sys.locale` holds one tag, and the
  locale list lives in the `system_locales` setting, which only
  `ActivityManager.updatePersistentConfiguration` (Settings'
  `LocalePicker`) writes. There is no shell command for it.
- The language follows the Mac at boot only, for the same reason.
- Once a language is picked in Android's Settings, `system_locales` is
  set and wins over `persist.sys.locale` at every boot.

## Verified (2026-09-29)

`cargo aim boot` with a new data image, the Mac on Asia/Seoul, `ko-KR`
and Light:

- `date`: KST, the Mac's time; `getprop persist.sys.timezone`
  `Asia/Seoul`, `persist.sys.locale` `ko-KR`; `am get-config`
  `ko-rKR-...-notnight`; Settings draws in Korean.
- Live, through the same property path (`setprop vendor.aim.mac.night_mode
  yes`, `vendor.aim.mac.time_zone America/Los_Angeles` from a guest
  shell): `cmd uimode night` `yes`, `ui_night_mode` 2, Settings redraws
  dark; `date` PDT, the status bar clock follows, and the detector's
  device zone is `America/Los_Angeles` with confidence 100.
- `cmd uimode night no` inside Android then held.
- The next boot of that data image, left dark and on
  `America/Los_Angeles` inside Android, started on `Asia/Seoul` (the
  detector's device zone too) and switched to light at
  `sys.boot_completed` (`am get-config` `notnight`).

Unit tests: `mac::tests` (zone paths, tzdata index, tag mapping, region,
appearance) and `boot_image::derived_image_tzdata_has_the_macs_zones`.
