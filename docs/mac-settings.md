# The Mac's time zone, language, appearance and keyboard layout

The device follows four of the Mac's settings (#283, #282, #281, #288). The
host reads them and the guest applies them through Android's own services,
the way a device's vendor init would; no part of Android is patched, and
the Mac's settings are only read.

| Setting | Mac source | Property | Applied by `init.aim.rc` |
| --- | --- | --- | --- |
| Time zone | the IANA name `/etc/localtime` links to, if the image's tzdata (`/apex/com.android.tzdata/etc/tz/tzdata`) has it | `vendor.aim.mac.time_zone` | at boot: `setprop persist.sys.timezone` in `post-fs-data`; while running: `cmd alarm set-timezone` (`AlarmManager.setTimeZone`) |
| Language and region | `CFLocaleCopyPreferredLanguages`, each language Android can name, with `AppleLocale`'s region when it has none (`ko` + `ko_KR` → `ko-KR`, `zh-Hans` + `zh_CN` → `zh-Hans-CN`) | `vendor.aim.mac.locale` (the first) | at boot: `setprop persist.sys.locale` in `post-fs-data`; the whole list: the service host, through the bridge ("The language list") |
| Appearance | `AppleInterfaceStyle` (`Dark`, or absent for Light; "Auto" updates it) | `vendor.aim.mac.night_mode` (`yes`/`no`) | `cmd uimode night` (`UiModeManager.setNightMode`, `ui_night_mode`), at `sys.boot_completed` and on each change |
| Keyboard layout | `AppleCurrentKeyboardLayoutInputSourceID` of `com.apple.HIToolbox` (`com.apple.keylayout.Dvorak`), as the InputDevices layout with its characters, else US English ([input.md](input.md), "Layouts") | `vendor.aim.mac.keyboard_layout` (`keyboard_layout_english_us_dvorak`) | `aim-keyboard layout` (InputManager's layout override for the built-in keyboard), at `sys.boot_completed` and on each change |

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
  with automatic detection on (the default, with the geolocation
  declaration) its capability is "not applicable".
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
- **The native uimode** ([system-services.md](system-services.md),
  "The uimode service") follows the appearance itself, from its start and
  on the Mac's change notification, so the `cmd uimode night` trigger
  repeats it (#655).

## The language list (#344)

`persist.sys.locale` holds one tag; the device's list lives in the
`system_locales` setting and the global configuration, which only
`ActivityManager.updatePersistentConfiguration` (Settings'
`LocalePicker`) changes, with no shell command for it. So the service
host applies the list through the system_server bridge
([system-services.md](system-services.md), "The system_server bridge"):

- **Host** (`crates/aim-services/src/locale/`). The Mac's preferred
  languages (`CFLocaleCopyPreferredLanguages`) mapped as in the table,
  each once (`LocaleList` refuses a repetition). A thread of its own
  runs a CFRunLoop for the distributed
  `AppleLanguagePreferencesChangedNotification` and CoreFoundation's
  `kCFLocaleCurrentLocaleDidChangeNotification` (NSCurrentLocaleDidChange).
- **Bridge.** `IBridge.updateLocales(tags)` runs the platform's
  `LocalePicker.updateLocales` in system_server (`LocaleBridge.java`):
  the configuration with `userSetLocale`, persisted to `system_locales`
  and `persist.sys.locale`, as Settings' language page does.
- **When.** Each time a bridge attaches (every boot, and again after
  system_server restarted), and on a notification whose re-read list
  differs from the last one seen. A notification that leaves the list as
  it was (another region setting) changes nothing.
- **Changes made inside Android** (Settings' language page, an app with
  `CHANGE_CONFIGURATION`) hold until the Mac's list changes or the
  device boots, as the appearance does (the user's decision on #344).
  Android never writes the Mac's languages.
- **At boot** the bridge attaches at `PHASE_DEVICE_SPECIFIC_SERVICES_READY`,
  before AppOpsService is ready, so ActivityManager refuses
  system_server's own persistent configuration change (`WRITE_SETTINGS`).
  The list waits for `PHASE_THIRD_PARTY_APPS_CAN_START`, which comes
  after the apps' data is prepared (`waitForAppDataPrepared`; at
  `PHASE_ACTIVITY_MANAGER_READY` a first boot started a receiver of the
  change without its data directory, #665) and before the persistent
  apps and home start. Until then the configuration is
  `persist.sys.locale`, the Mac's first language, or a list chosen in
  Android before.

## Verified: the language list (2026-10-01)

Device boots of the working tree with the Mac on `("ko-KR")`, `ko_KR`
(the Mac's language was not changed; checks locale-1..4):

- A first boot of a new data image: `AimLocaleBridge: languages set to
  the Mac's: ko-KR` once, 13 ms after `OnBootPhase_600`, no permission
  denial; `system_locales` `ko-KR` (unset before), `persist.sys.locale`
  `ko-KR`, `am get-config` `ko-rKR`; Settings' language page lists
  한국어 (대한민국); the crash buffer empty.
- `settings put system system_locales en-US,ja-JP`, then a boot of the
  same data: `system_locales` `ko-KR` again, the Mac's.
- CtsLocaleManagerTestCases: OK (31 tests); its own system locale
  changes held afterwards (no further bridge call).
- A live change of the Mac's list is not checked in a boot (it would
  change the Mac's settings): its handling is unit-tested, and it takes
  the same `IBridge.updateLocales` call.

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

Unit tests: `mac::tests` of aim-guest-init (zone paths, tzdata index,
appearance), `locale::tests` and `locale::mac::tests` of aim-services
(tag mapping, region, the list, when it applies, the Mac's list read and
a change heard) and `boot_image::derived_image_tzdata_has_the_macs_zones`.
