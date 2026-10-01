# The shell in window mode (ADR 0013, M1)

The first step of ADR 0013's order (decision 5.1): in window mode
([windows.md](windows.md)) the Android shell does not run: no launcher, no
SystemUI and the WMShell inside it, no keyguard, no wallpaper. macOS is the
shell (Dock, shims, Spotlight, the menu bar, Notification Center), and apps
keep their behavior. This document is the design and the plan; the work is
tracked in #463.

Evidence is from the pinned sources (`android-16.0.0_r1`,
`platform/frameworks/base`; paths below are relative to it, `wm/` is
`services/core/java/com/android/server/wm/`, `am/`, `policy/` likewise), the
derived image (`target/aim/derived/root`) and boot logs of 2026-09-29/30
(first boots at load 5-20, settled boots of a reused data image).

## Summary

- **Boot completion needs a home activity, nothing else of the shell.**
  An empty HOME activity of the device's own (`java/lightweight-home`)
  is one; the platform's `SystemUserHomeActivity` is not, since
  ActivityTaskManager does not resolve it. The keyguard is absent when its service cannot be
  bound, which the platform handles. One resource must change:
  `config_checkWallpaperAtBoot`, else the screen waits 30 s for a wallpaper
  nobody draws. No SystemServer change.
- **WMShell is not needed for freeform windows.** Android 16 has no legacy
  transitions, but with no transition player and no task organizer
  registered, WindowManager applies every change at once and positions and
  crops task surfaces itself. aim-windows keeps its task calls; the caption
  and the bar margin go. Lost without a replacement: splash screens and
  PiP (decisions D3, D4).
- **What apps can observe of SystemUI** is mostly behind one callback
  interface, `IStatusBar`, which a native service can register, and a
  notification listener. Must-haves before the switch: notification
  clicks and actions, text toasts with their callbacks, the MediaProjection
  consent activity, the authentication dialog when a credential exists,
  screen pinning.
- **Boot time** gains little on the critical path of a settled boot
  (estimated 0.3-0.8 s of 13.7 s), more on first boots and under load, and
  most after boot: four fewer app processes and SystemUI's and the
  launcher's start-up work off the CPU while the first app starts.

## 1. Boot completion without a home screen, keyguard or wallpaper

`sys.boot_completed` is set in `ActivityManagerService.finishBooting`
(`am/ActivityManagerService.java:5162-5243`). The chain and its gates:

| Gate | Where | Waits for | Without the shell | Minimal supported answer |
| --- | --- | --- | --- | --- |
| Home launch delay | `am/ActivityManagerService.java:5357-5432`, `RootWindowContainer.java:1384` | SystemUI's ThemeOverlayController (`setThemeOverlayReady`), 15 s timeout | the image's flag is off: "ThemeHomeDelay: Home launch is not delayable" in every log | nothing |
| Home start | `am/ActivityManagerService.java:9116-9137` (`systemReady`) | a resolvable HOME activity; none: `startHomeOnTaskDisplayArea` returns false (`wm/RootWindowContainer.java:1376`) and nothing else starts one | no launcher: Settings' `FallbackHome` (priority -1000) is home; once the user is unlocked it sees another HOME and finishes, and ActivityTaskManager starts it again, hundreds of times a second | an empty HOME activity with `CATEGORY_DEFAULT` (`java/lightweight-home`, direct boot aware, over the wallpaper as a launcher): `RootWindowContainer.resolveHomeActivity` resolves with `MATCH_DEFAULT_ONLY` (`wm/ActivityTaskSupervisor.java:768-769`), so the platform's `SystemUserHomeActivity` (`ro.system_user_home_needed`; `core/res/AndroidManifest.xml:9197`, no DEFAULT) never matches, while `PackageManager.resolveActivity` without that flag, FallbackHome's check, finds it |
| Home idle | `wm/ActivityTaskSupervisor.java:1474-1516` | all resumed activities idle, then `postFinishBooting` (`wm/ActivityTaskManagerService.java:5367`) | as above | the home goes idle after its first frame |
| Keyguard drawn | `policy/PhoneWindowManager.java:6668` (`canDismissBootAnimation`), `:6320-6335` | the keyguard's first draw, 1 s (5 s before boot) timeout | `KeyguardServiceDelegate.bindService` fails, sets `deviceHasKeyguard=false` (`policy/keyguard/KeyguardServiceDelegate.java:160-173`), and screen-on reports keyguard drawn at once | nothing |
| System windows drawn | `wm/DisplayContent.java:4893-4966` (`shouldWaitForSystemDecorWindowsOnBoot`) | a drawn app window (the notification shade counts as drawn when absent), and a drawn wallpaper if `config_enableWallpaperService` and `config_checkWallpaperAtBoot` | both configs are true in the image (no RRO overrides them); ImageWallpaper is SystemUI's, so no wallpaper window: the wait ends at `BOOT_TIMEOUT`, 30 s (`wm/WindowManagerService.java:3844`) | a static RRO with `config_checkWallpaperAtBoot=false` (keeps WallpaperManagerService and its API) |
| Boot animation | `wm/WindowManagerService.java:3931-3985` | `bootanim` to stop | not run (`debug.sf.nobootanimation`) | nothing |
| SystemUI start | `services/java/com/android/server/SystemServer.java:3493, 3646-3653` | nothing: `startServiceAsUser` of a missing component returns null, then `onSystemUiStarted` binds the keyguard (fails as above) | harmless | nothing |

What `finishBooting` and `performEnableScreen` do must all still happen,
and does, since the chain is the original's: input dispatch is enabled
(`wm/WindowManagerService.java:3955`), `SurfaceControl.bootFinished`, the
ART runtimes' `bootCompleted` in zygote and system_server, the storage
checkpoint commit, `PHASE_BOOT_COMPLETED` to every system service
(JobScheduler, alarms, backup, ...), processes on hold started,
`sys.boot_completed` and `dev.bootcomplete` (init triggers, keystore2),
and `UserController.onBootComplete`: `LOCKED_BOOT_COMPLETED` and
`BOOT_COMPLETED` to apps (WorkManager, alarms re-armed, GMS).

**One image for both modes** (D1): the derived image leaves the shell
out, and device mode is a debugging view of the display without an
Android shell; nothing depends on the mode. SystemUI is
`android:persistent` (`packages/SystemUI/AndroidManifest.xml:409`), so
AMS starts it whatever SystemServer does; only its absence keeps it from
running. Until the switch this is a check-only image variant, `cargo aim
build --variant lightweight-shell` ([build.md](build.md), "Image
variants"), whose entries (`image/variants/lightweight-shell.toml`) move
into `image/overlay.toml` with the switch:

| What | Mechanism |
| --- | --- |
| The shell packages | removed: SystemUIGoogle, NexusLauncherRelease, WallpaperPickerGoogleRelease and ThemePicker (the wallpaper and style pickers), QuickAccessWallet (a plugin of SystemUI's global actions) |
| The home | `java/lightweight-home`, `/system_ext/app/AimHome`: an empty HOME activity over the wallpaper (section 1's table) |
| The static wallpaper | `java/image-wallpaper`, a privileged app in `/system_ext/priv-app` with READ_WALLPAPER_INTERNAL: a `WallpaperService` drawing the current bitmap as SystemUI's ImageWallpaper does; the overlay names it in `image_wallpaper_component`, so WallpaperManagerService's static wallpaper, clear and default keep their meaning (#603) |
| Resource values (the static wallpaper and its check, shell-only service configs) | a static RRO, `java/lightweight-shell-overlay`, in `/system_ext/overlay`: framework-res declares no overlayable, so a preinstalled overlay may override its configs (`cmds/idmap2/libidmap2/ResourceMapping.cpp:60-75`), and `/system_ext` overlays take precedence over `/product`'s (`PackagePartitions`), where `PixelConfigOverlayCommon` names the shell-only services |
| No caption | `/vendor/etc/init/aim-lightweight-shell.rc`: `ro.vendor.aim.freeform_caption_dp=0`, which aim-windows reports ([windows.md](windows.md)) |
| The native status bar | the same rc: `ro.vendor.aim.lightweight_shell=true`, on which guest-init starts the native `IStatusBar` (section 3) |

## 2. WMShell's roles

WMShell (`libs/WindowManager/Shell`) runs in SystemUI's process and
registers the transition player (`transition/Transitions.java:396`) and the
task organizer (`ShellTaskOrganizer.java:275`). What WindowManager does
without them (files in `wm/` unless named otherwise):

- **No legacy transitions.** `wm/` has no `AppTransition` or
  `RemoteAnimationController`; `isShellTransitionsEnabled()` means "a player
  is registered" (`wm/TransitionController.java:384-386`), and no property
  or config enables it. With none, transition requests return null
  (`:766, :875`), creation paths check first, visibility is committed at
  once (`ActivityRecord.java:5490`), and `resizeTask` resizes directly
  (`ActivityTaskManagerService.java:2969-2977`). A transition that becomes
  ready without a player is applied and finished by WM itself
  (`Transition.java:2016-2025, 2273-2284`). This is the state every boot
  runs in until SystemUI registers.
- **No organizer: WM owns task surfaces.** Position and crop are skipped
  only for organized tasks (`WindowContainer.java:3211-3215`,
  `Task.java:2334-2336`); there is one organizer for all root tasks, not
  one per windowing mode (`TaskOrganizerController.java:569`).
- **Apps still get `onEnterAnimationComplete`** when an activity becomes
  visible without a transition (`ActivityRecord.java:5560-5572`).

| Role | Without WMShell | Apps see | M1 |
| --- | --- | --- | --- |
| Task organizer | WM positions, crops and shows task surfaces | nothing | none registered; aim-windows keeps its `IActivityTaskManager` calls; its "a pixel and back" commit of bounds (windows.md, "Placement") is checked and removed if WM now places surfaces itself |
| Transition player | changes apply at once, no animation | no in-task activity animations, `overridePendingTransition` has no effect | none; the Mac animates windows |
| Freeform caption (window decoration) | none, no caption insets | a task's content starts at its top | aim-windows reports caption 0; the title bar sits above the task, not over it (#351 moot) |
| Bars and taskbar | no status, navigation or task bar windows (`DisplayPolicy.java:1081-1144`) | zero system bar insets | the display is the screen, without `BAR_MARGIN` |
| Starting windows (splash) | `addStartingWindow` needs an organizer (`TaskOrganizerController.java:629-632`, `StartingSurfaceController.java:83-92`); none is drawn | no splash; `SplashScreen.OnExitAnimationListener` is not called (the API allows a start without splash) | D4 |
| PiP | with `enablePip2` (phones, not ARC or TV: `ActivityTaskManagerService.java:7601-7612`) entry is a transition that aborts without a player (`:3835-3855`); otherwise a pinned task without bounds policy | `enterPictureInPictureMode` returns true and nothing happens | D3 |
| Split screen, bubbles | not offered | multi-window apps run as freeform windows | none |
| Predictive back | no `startBackNavigation`; the Back key goes through `ActivityClientController.onBackPressed` (`:1720-1745`) and apps' `OnBackInvokedCallback`s | no back preview animation | none (Back is `KEY_BACK`, windows.md) |
| Desktop mode | off (`config_isDesktopModeSupported` false, not overridden) | nothing | none |

**The alternative, if D3 or D4 or CTS call for it** (step 8): a native task
organizer and transition player. Registering either changes ownership: the
organizer receives every root task and must place their leashes; the player
must apply each transition's start transaction, show the opening leashes
at their end bounds and apply the finish transaction before
`finishTransition` (what `Transition.cleanUpOnFailure` does). The parcels
carry `SurfaceControl`s and `SurfaceControl.Transaction`s, which only
libgui (C++, not the NDK) or Java can read and apply, so this piece cannot
be Rust over the NDK alone (D5).

## 3. What else SystemUI provides that apps can observe

With SystemUI gone, system_server null-checks its callback (`mBar` in
`StatusBarManagerService`) and binding failures almost everywhere: apps get
silence, rarely an error. Most rows below are one interface, `IStatusBar`,
registered with `IStatusBarService.registerStatusBar` (STATUS_BAR_SERVICE;
the native service host has the system uid). A native `IStatusBar` is
registered once SystemUI no longer runs (registration replaces, so it
cannot coexist with SystemUI). "Keep SystemUI headless" is not an option:
SystemUI has no configuration to start without its UI, and its
`Application` builds WMShell.

| Feature | Path | Without SystemUI | M1 answer |
| --- | --- | --- | --- |
| Text toasts (Android 11+) | `Toast.java:199-229` → `enqueueTextToast` → `StatusBarManagerService.showToast` (`:876-886`) → `IStatusBar.showToast` | no toast, no error; `Toast.Callback` never called (only the renderer calls `ITransientNotificationCallback`, `ToastPresenter.java:258, 286`) | native: a non-activating Mac panel at the app's window; the service calls `onToastShown`/`onToastHidden` (also fixes #349 for toasts) |
| Custom-view toasts | the app adds the window | unchanged | none |
| Notification clicks, actions, direct reply, clear | NMS only cancels on click (`NotificationManagerService.java:1352-1396`); SystemUI sends the `PendingIntent` and reports back | nothing happens | native notification listener (system uid, `INotificationManager.registerListener`) → `UNUserNotificationCenter`; click and actions send the `PendingIntent` and report `onNotificationClick`/`ActionClick`/`Clear` (#4) |
| Full-screen intents, heads-up | SystemUI launches the FSI (`StatusBarNotificationActivityStarter.java:700-716`) | not launched | the listener: an FSI as a time-sensitive Mac notification that opens the intent |
| Media controls | SystemUI's media carousel from media-style notifications | not shown (MediaSession works) | Mac Now Playing (`MPNowPlayingInfoCenter`, `MPRemoteCommandCenter`) of the session media keys go to, in the app's shim ([media.md](media.md)) |
| Bubbles | WMShell | not offered | none (the notification shows normally) |
| Volume controller, safe-volume warnings | `AudioService.setVolumeController` (`:13030-13035`), posts are no-ops without one (`:13214-13280`) | volumes change, no UI, warnings dropped | none: the media volume is the Mac's output volume, whose own UI shows it (D7, [audio.md](audio.md) "Volume"); no safe-volume warning on the speaker |
| Output switcher | `showMediaOutputSwitcher` returns true, no-op (`StatusBarManagerService.java:993-1000`) | nothing | step 5 (the Mac's output picker) |
| MediaProjection consent | `MediaProjectionManager.createScreenCaptureIntent` targets `config_mediaProjectionPermissionDialogComponent` (SystemUI's activity) | `ActivityNotFoundException`: no screen capture | an activity of ours at that config (RRO), with a Mac consent sheet on the app's window ([media.md](media.md)) |
| `Activity.ScreenCaptureCallback` | only SystemUI calls `WindowManagerService.notifyScreenshotListeners` (`:10239-10244`) | never called | native: on a Mac screenshot of an app window (no official macOS event; step 5) |
| Screenshots (key chord) | `ScreenshotHelper` binds `config_screenshotServiceComponent`, 10 s timeout, error broadcast | no Android screenshot | none: macOS screenshots |
| BiometricPrompt, device credential prompt | `AuthSession` → `IStatusBar.showAuthenticationDialog` | with a credential: no dialog, the app never gets a result; without credential or biometrics BiometricService fails the request before any UI | native: a Mac sheet verifying through `ILockSettings`, D2 |
| Keyguard APIs | bind fails: not showing, not secure (`KeyguardServiceDelegate.java:160-173`) | `isKeyguardLocked` false, `requestDismissKeyguard` gets `onDismissError`, `isDeviceLocked` false | correct for a Mac (its login is the lock), D2 |
| Screen pinning | `LockTaskController.java:658-664` → `showScreenPinningRequest`; unpinning is the navigation bar's Back and Overview held → `stopSystemLockTaskMode` | `startLockTask` silently does not pin | native confirm sheet → `startSystemLockTaskMode`; while pinned (`showPinningEnterExitToast`), a pin in the app's menu bar whose menu unpins (#586) |
| `StatusBarManager.expandNotificationsPanel`, `collapsePanels` | no-ops without `mBar` | nothing (also nothing to expand) | native `IStatusBar`: open/close Notification Center is not possible from an app; stays a no-op, as on a device without a shade |
| Quick Settings tiles (`TileService`), `requestAddTileService` | bound only by SystemUI; request answers `TILE_ADD_REQUEST_ERROR_NO_STATUS_BAR_SERVICE` (`:2494-2513`) | tiles never bound | D8: the native `IStatusBar` answers a dismissed dialog (the app hears "not added", no denial recorded) |
| IME picker | system_server's dialog (`InputMethodMenuController.java:79-199`) | unchanged | none (IME windows themselves: #23) |
| Recents, `ActivityManager.AppTask` | ActivityTaskManager | unchanged | none; the recents key maps to nothing |
| Wallpaper API | files and colors in system_server (`WallpaperManagerService.java:546`) | unchanged; the device's static wallpaper (`java/image-wallpaper`) draws it, which nothing shows on the Mac | none (#603) |
| Dynamic color (Material You) | SystemUI's ThemeOverlayController writes the palette overlays | apps get the image's default palette | step 6: the palette from the Mac's accent color, as `FabricatedOverlay`s through `IOverlayManager` |
| Clipboard overlay | SystemUI's `ClipboardListener` | none (#434 gone) | none |
| Window magnification (accessibility) | `requestMagnificationConnection` → SystemUI | window magnification unavailable, full-screen magnification works | issue after step 4 |
| USB permission, slice permission, sensor-privacy unblock | SystemUI activities | not shown; requests unanswered | only if the device exposes USB devices or sensor-privacy toggles; issues then |
| Global actions, shutdown UI | with an `IStatusBar` registered, GlobalActions asks it to show the power menu and falls back to system_server's own after 5 s without `onGlobalActionsShown` (`policy/GlobalActions.java:59-111`); ShutdownThread leaves its dialog to the bar while `config_showSysuiShutdown` (`StatusBarManagerService.java:711-723`) | system_server's own power menu and shutdown dialog | the native `IStatusBar` shows neither: the power menu (reached by a long press of a power key the Mac does not have, or an accessibility service's `GLOBAL_ACTION_POWER_DIALOG`) is system_server's after the 5 s, and the overlay sets `config_showSysuiShutdown=false`; both are system windows without a task, which window mode does not show (#349). The Mac's own menu and `aimctl` power Android off (#585) |

## 4. What starts only for the shell

| Component | Started by | Evidence | After M1 |
| --- | --- | --- | --- |
| `com.android.systemui` (SystemUIService, KeyguardService, ImageWallpaper, WMShell) | persistent app, ImageWallpaper binding | "Start proc ...:com.android.systemui for service {...ImageWallpaper}" then "for added application" right after `boot_progress_ams_ready`; ThemeOverlayController's first run: "Slow dispatch took 3236-6881ms SysUiBg" | not started |
| `com.google.android.apps.nexuslauncher` (Quickstep: home, recents, taskbar) | SystemUI binds its `TouchInteractionService`; HOME | "Start proc ...:nexuslauncher for service {...TouchInteractionService}", "Displayed ...NexusLauncherActivity +7s359ms" on a first boot; 0.9 cores two minutes after boot ([perf-baseline.md](perf-baseline.md)) | not started |
| `com.google.android.apps.wallpaper` | its `RecentWallpapersProvider`, queried at boot by the shell | "Start proc ... for content provider {...RecentWallpapersProvider}" 1-2 s after boot | not started (to confirm with the process list in step 4) |
| `com.google.android.googlequicksearchbox:search` | `PublicSearchService`, bound by the launcher's search; and the default assistant's `:interactor` (`GsaVoiceInteractionService`, bound by VoiceInteractionManagerService), which binds its own `SearchProcessEndpointService` there | "Start proc ...:search for service {...PublicSearchService}"; in the lightweight shell, `SearchProcessEndpointService` bound from `:interactor` (#604) | stays, with `:interactor`, while the Google app holds the assistant role |
| SearchUi, Smartspace, ContextualSearch, WallpaperEffectsGeneration, ContentSuggestions managers | SystemServer, because `PixelConfigOverlayCommon` sets their `config_default*Service` (`SystemServer.java:2068-2112, 2396-2400`) | each 2-6 ms in `SystemServerTiming`; clients are the launcher and SystemUI | off in window mode by the same configs (step 7); AppPrediction stays (the share sheet uses it) |
| StatusBarManagerService, WallpaperManagerService, TrustManagerService, DreamManagerService | SystemServer | apps call their APIs | stay |
| added: `dev.aim.home` | the home | | one small app process |

Not the shell but found on the way: the emulator's persistent
`com.android.emulator.multidisplay` runs on every boot (#462).

## 5. Plan

Each step lands with apps working in both modes. Verification: the
integration gate and the app checks (Settings, Chrome, Calculator, the
tracked game set) in window mode every time, and the CTS modules listed,
run once before (the original, window mode with SystemUI) and after, as in
[system-services.md](system-services.md). Boot gains are estimates against
13.7 s: on the settled boot of 2026-09-30, 08:17, `boot_progress_ams_ready`
came about 1 s before `wm_boot_animation_done` (launcher process 0.57 s
after it, its launch 226 ms, enable-screen to done 0.13 s); on first boots
at load 5-20 the same span was 17-18 s, with 5.2 s from enable-screen to
done and the launcher's first frame after 7.4 s.

| # | Step | Apps keep | Verification | Estimated gain |
| --- | --- | --- | --- | --- |
| 1 | **Mode and RRO.** guest-init sets the SKU from its command line; a `cargo aim` node writes a signed resource-only overlay (binary manifest and `resources.arsc`; aim-apps already reads both); `config_checkWallpaperAtBoot=false` in both modes | everything; the screen no longer waits for ImageWallpaper's first frame | boots of both modes; `cmd overlay list`; CtsWallpaperTestCases | settled: ≤0.1 s; first boot under load: up to the 5 s from enable-screen to done |
| 2 | **Notifications on the Mac** (#4): a native listener in the service host posts to `UNUserNotificationCenter`; click, actions, direct reply (`RemoteInput`), clear, FSI. Done except full-screen intents (#469) and custom views (#468) ([notifications.md](notifications.md)) | notifications become visible in window mode and actionable; SystemUI's shade runs unseen alongside | CtsNotificationTestCases, CtsLegacyNotification* (NMS unchanged: no regressions); an app posting each kind; Chrome's download notification | none (lazy) |
| 3 | **SystemUI's activities apps start:** MediaProjection consent as our activity behind the config (RRO), then USB permission if the device exposes USB devices. The consent is in place ([media.md](media.md)), with Now Playing | screen capture keeps working when SystemUI goes | CtsMediaProjectionTestCases, CtsMediaProjectionSDK33/34TestCases; a screen-sharing app | none |
| 4 | **The switch.** In window mode: no shell packages (D1), an empty home, a native `IStatusBar` (toasts with callbacks, authentication dialog (D2), screen pinning, and every other method classified in the service), aim-windows without caption and bar margin, PiP per D3 | all of the above; Home per D6 | CtsWindowManagerDeviceActivity, -Am, -Window, -Insets, -Keyguard, -BackNavigation, -Animations, -Other; CtsToastTestCases, CtsToastLegacyTestCases, CtsWidgetTestCases (ToastTest); CtsAppTestCases (StatusBarManager, KeyguardManager, ActivityManager); CtsBiometricsTestCases (credential paths); CtsRoleTestCases (HOME); CtsTileServiceTestCases (the request result); CtsSystemUiTestCases (expected failures listed); CtsInputMethodTestCases; the process list and a logcat without new WTFs | settled: 0.2-0.5 s off `sys.boot_completed` (an empty home instead of the launcher's cold start); first boot under load: several seconds; after boot: four fewer app processes, SystemUI's 3-7 s start-up dispatches and the launcher's CPU gone before the first app start |
| 5 | **Volume, output, screenshots:** the speaker as an absolute-volume device following the Mac's output volume (D7, done: [audio.md](audio.md) "Volume"), the output switcher, `notifyScreenshotListeners` | the media volume on the Mac's keys and menu bar, screenshot callbacks | CtsMediaAudioTestCases (AudioManager volume classes); the ScreenCaptureCallback tests of CtsWindowManagerDevice* | none |
| 6 | **Dynamic color from the Mac:** the accent color (`NSColor.controlAccentColor`, and changes) as the system palette overlays, as mac-settings.md does for appearance | Material You colors that follow the Mac | CtsGraphicsTestCases (system palette), CtsThemeDeviceTestCases | none (an overlay commit only on change) |
| 7 | **Shell-only services off** in window mode (their configs by RRO) | nothing uses them without the shell | smoke boot; CtsWallpaperEffectsGenerationServiceTestCases and the like skip as on devices without them (counted as not run) | < 50 ms |
| 8 | **Only if D3, D4 or step 4's CTS require it:** native task organizer and transition player (§2), splash screens and PiP as Mac windows. | splash screens, PiP | step 4's WindowManager modules and PiP tests | none |

Steps 2 and 3 come before the switch because they can run beside SystemUI
and the switch would otherwise break notification clicks and screen
capture. Everything in step 4 must land together: the native `IStatusBar`
cannot coexist with SystemUI's, and without it the switch loses toast
callbacks.

## Decisions

| | Question | Options | Recommendation |
| --- | --- | --- | --- |
| D1 | How window mode leaves the shell packages out, and whether the modes share a data directory | (a) the shell is removed from the image for both modes (device mode shows the display without an Android shell); (b) a mode-conditional `remove` in image/overlay.toml that guest-init applies by path map for the SKU: switching modes on one data directory makes PackageManager drop and re-add them (the launcher's layout, SystemUI's settings and the HOME role are reset); (c) as (b) with one data directory per mode | decided (#463): (a), no SKU for the mode |
| D2 | The Android lock credential in window mode | no Android credential (the Mac login is the lock; a PIN set in Settings leaves user 0's credential-encrypted storage locked at the next boot, with no unlock UI); or a native unlock and credential sheet (step 4) | the native sheet, since Settings lets users set a PIN |
| D3 | Picture-in-picture | declare `android.software.picture_in_picture` unavailable in window mode (apps check it); or step 8 with a floating Mac window | unavailable in M1, step 8 later |
| D4 | Splash screens | none; the Mac window shows the app's icon on its splash background until the first frame (aim-apps draws the icons); or step 8 | decided (#463): the shim's window appears at once with the app's icon until the first frame (`startActivityAndWait`, [windows.md](windows.md), "App shims"); step 8: the window shell draws Android's starting windows, and the Mac's splash covers only the time before the task's window ([task-organizer.md](task-organizer.md), "Starting windows") |
| D5 | Language of the Android-side replacements that must be Android components (the consent activity, an organizer and player) | Java app / `app_process`; Rust `NativeActivity` over JNI; thin C++ over libgui | Rust where the NDK reaches, the rest decided per piece |
| D6 | What an app's "go home" (a HOME intent) does on the Mac | nothing visible (the empty home takes focus); hide the app's windows; show the desktop | decided (#463): the app hides, as with Cmd+H ([windows.md](windows.md), "Home") |
| D7 | Volume | Android's stream volumes stay Android's, with a Mac-style HUD; or tie the music stream to the Mac's output volume | decided (#463): the media volume is the Mac's output volume, both ways, no HUD, mute follows the Mac |
| D8 | Quick Settings tiles | a menu-bar extra that binds apps' `TileService`s; or none (`requestAddTileService` answers "not added") | none in M1 |

ADR 0013's "Steps" section already names the clipboard pipeline "M1" (its
commit called it M0); this document uses M1 for decision 5.1, as #463 does.
