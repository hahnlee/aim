# Window mode (#294, #248)

In window mode each Android app task is its own macOS window, with native
chrome, over one resident Android instance, as Windows Subsystem for
Android and ChromeOS show Android apps. Device mode, one window showing the
whole display ([composer.md](composer.md)), stays the default.

```sh
cargo aim boot --windows            # aim-display --mode windows
```

```text
 Mac screen (points)                        Android display (pixels)
 ┌────────────────────────────┐             ┌────────────────────────────┐
 │ menu bar                   │             │ status bar                 │
 │   ┌─title bar─┐            │   s·(x,y)   │   ┌─caption──┐             │
 │   │ Settings  │ ┌─────┐    │ ──────────▶ │   │ task 9   │ ┌──────┐    │
 │   │  content  │ │Chrome│   │             │   │ content  │ │task 11│   │
 │   └───────────┘ └─────┘    │             │   └──────────┘ └──────┘    │
 └────────────────────────────┘             ├────────────────────────────┤
                                            │ bar margin (nav/taskbar)   │
                                            └────────────────────────────┘
```

## Design: the display is the screen

The original freeform windowing gives every task bounds of its own on the
default display. Window mode makes that display **congruent with the Mac's
main screen**: display pixel (x, y) lies under screen point
(x, H·s − y) / s for a screen H points tall at s pixels per point, and each
task's window lies exactly over the task. The rest follows from that:

- **Composition stays the original's.** SurfaceFlinger composes the
  display as in device mode (client composition), and each window's layer
  shows its task's part of the frame, one pixel per pixel. Nothing in the
  composer HAL changed. Where two windows overlap, Android stacks the tasks
  as the Mac stacks the windows (below), so what one window shows of
  another task is covered by that task's own window on screen.
- **Input stays one touchscreen.** A press in a window is a touch at the
  display pixel under it, which Android's own hit testing sends to the
  task (and its dialogs and popups) there.
- **Android pixels and macOS points stay apart.** The display is the
  screen in backing pixels, never an upscale; `aim_host_display::windows`
  is the one place that converts.
- **No caption.** The image has no WMShell (the lightweight shell,
  [m1-shell.md](m1-shell.md)), so no window decoration draws a freeform
  caption: `ro.vendor.aim.freeform_caption_dp` is 0 and the window has a
  standard title bar (#545); a caption height above 0 would lie inside
  the top of the task, under a title bar made at least as tall. The
  display still has `BAR_MARGIN` rows below the screen, for bars nothing
  draws any more (#681).

## Pieces

| Piece | Where |
| --- | --- |
| Geometry (screen points ↔ display pixels, content, caption) | `crates/aim-host-display/src/windows.rs` |
| Task windows (AppKit), back hint | `crates/aim-host-display/src/bin/aim-display/windows.rs` |
| Presenting a frame into several layers, each a crop | `bin/aim-display/metal.rs` (`Target`) |
| Task bridge (guest, Rust): task listener and task calls | `daemons/windows` (`/system_ext/bin/aim-windows`) |
| `RunningTaskInfo` parcel reading | `daemons/windows/core` |
| Host-call `FN_WINDOWS`, records | `aim_hostcall::display::{Windows, Window, window}` |
| Freeform support | `/vendor/etc/permissions/android.software.freeform_window_management.xml` |
| Window hosts (the server's side; a shim's side) | `bin/aim-display/hosts.rs`, `bin/aim-display/shim.rs` |
| APK labels and icons, macOS icons, shim bundles | `crates/aim-apps` (`aim-apps`) |

## The task bridge

`aim-windows` is a system daemon (`/system_ext/etc/init/aim-windows.rc`,
`user system`) that speaks the platform's own binder interfaces:
`IActivityTaskManager` (`registerTaskStackListener`, `getTaskBounds`,
`resizeTask`, `setFocusedTask`, `removeTask`, `startActivityAndWait`),
`IWindowManager` (`get`/`setWindowingMode`, base display size and density)
and the `ITaskStackListener` it registers (tasks, focus and
`onActivityRequestedOrientationChanged`). These are Java AIDL interfaces
with no NDK backend, so the few calls are written by hand with the
transaction codes of the pinned AIDL (first method = 1); the image's
`framework.jar` stubs have the same `TRANSACTION_*` values. The
`RunningTaskInfo` of the callbacks is read up to its `TaskDescription`
label, in the layout of the pinned `TaskInfo`, `Intent` (with
`prevent_intent_redirect`, which the image enables), `ComponentName`,
`Uri` and `Bundle`.

It connects to the display server with `FN_WINDOWS`, which answers the
server's mode. Its binders die with system_server, so its rc starts it
again when zygote restarts (`init.svc.zygote=restarting`); the server
drops the old connection's tasks. The mode:

- **Device mode:** it sets the default display's windowing mode to
  fullscreen (`setWindowingMode` persists, so a window-mode boot is undone)
  and exits.
- **Window mode:** it sets freeform and reports tasks. A task gets a window
  once an activity of it ran (it came to the front or had focus) and it has
  bounds of its own on the display. A task that fills the display (home,
  anything not freeform), a task without activities (WMShell's split-screen
  roots) and a task restored from recents but not started get none.

| Record | From | Meaning |
| --- | --- | --- |
| `TASK` | guest | the task's bounds and caption height, new or changed |
| `PACKAGE`, `TITLE` | guest | the task's package; its `TaskDescription` label |
| `ACTIVITY` | guest | the activity the task was started with, `package/class` (`origActivity`, an alias, else `realActivity`) |
| `FRONT` | guest | the task is the focused, top one |
| `MOVED_TO_BACK` | guest | Back on its root activity moved it behind the others |
| `ORIENTATION` | guest | an activity of it asked for landscape, portrait or neither (`setRequestedOrientation`) |
| `REMOVED` | guest | the task is gone |
| `HIDE` | guest | the desktop (a task that fills the display: home) came to the front over the task: its app hides |
| `DRAWN` | guest | the `LAUNCH` of `package/class` drew its first frame, or ended (with the window shell: its task's window shows its starting window) |
| `PINNED`, `UNPINNED` | guest | the task entered or left picture-in-picture: its window floats, or no longer ([task-organizer.md](task-organizer.md), "Picture-in-picture") |
| `SET_BOUNDS` | server | move or resize the task (`resizeTask`); the bridge answers with `TASK` |
| `FOCUS` | server | `setFocusedTask` |
| `CLOSE` | server | `removeTask` |
| `LAUNCH` | server | start a package's launcher activity in a new task (`startActivityAndWait`, in a thread of its own; `DRAWN` when it returns) |

## Windows

- **Placement.** A new task's window takes the bounds Android chose
  (freeform's cascade). When the screen does not allow them (above the
  menu bar), the task follows the window. Bounds Android gives a task
  itself (its launch position; a shift away from another task when an
  activity starts in it) do not always reach the task's surface: the
  legacy freeform transitions can leave it where it was, and the window
  would show another part of the display. The bridge commits them by
  moving the task a pixel and back with `resizeTask`, whose change
  transitions place the surface (a resize to the bounds the task already
  has changes nothing). With the window shell (the image's, since the
  lightweight shell) its transitions place every task's surface, and
  nothing is committed ([task-organizer.md](task-organizer.md)).
- **Where a window reopens.** A launcher activity's new task opens where
  its window last was, as a Mac app's window does (#635): the bridge keeps
  each activity's last bounds in `/data/system/aim-windows-places` and
  gives them to a `LAUNCH` as launch bounds while they lie within the
  display. Starts from elsewhere (an app's own new task, a notification)
  open at Android's default bounds, as on a desktop-windowing device. On a
  freeform display area (no window shell) the platform's own record
  (`LaunchParamsPersister`) agrees.
- **Move and resize.** Moving the window moves the task at once; a live
  resize resizes it when the resize ends (each resize is a configuration
  change for the app). The window then takes the bounds the task got (a
  minimum size), once the user has not moved it for half a second; bounds
  Android gives the task itself it takes at once. While they differ, the
  layer shows the task one pixel per pixel from the top left, unstretched.
- **Focus and stacking.** The key window's task is the focused one
  (`FOCUS`), and a task Android brings to the front makes its window key
  while the app is active. A press in a window whose task is not in front
  focuses the task first and holds the touch until Android reports it in
  front (250 ms at most), so it lands on that task.
- **Orientation.** An activity's `setRequestedOrientation` of a
  landscape or portrait orientation (`ActivityInfo.isFixedOrientation*`)
  turns its window's content to those proportions, its width and height
  swapped around its centre, fitted into the screen's visible part (never
  full screen); a request for neither (unspecified, sensor, user, ...)
  gives it back the size the user gave it, unless the user resized it
  since. The task follows the window as after the user's resize. A
  manifest's `screenOrientation` needs nothing of the window: freeform
  launches such a task with bounds of that orientation
  (`TaskLaunchParamsModifier`). An activity of a fixed manifest
  orientation that starts in a running task (or comes to its top when the
  one above finishes) does not turn the window (#593).
- **System bars.** Apps in window mode see no status or navigation bar:
  the platform gives a floating (freeform) task's windows only caption and
  IME insets (`InsetsPolicy`), and while a freeform task is visible it
  shows the system bars itself (`DisplayPolicy.updateSystemBarsLw`), so an
  app's immersive request (`WindowInsetsController.hide`) changes nothing.
  They still get the caption bar's inset (the freeform caption, 42 dp).
- **Minimize and close.** A minimized window's task goes behind the
  visible windows' tasks (they are focused back to front, in the screen's
  order). Closing a window removes its task; a removed task closes its
  window. Zoom (the green button) resizes; full screen is off.
- **Title.** The task's `TaskDescription` label, else what a launcher
  calls it: in a shim, the shim's name; in the server, the label of the
  launcher activity the task was started with, else its app's label (both
  from the shims in `--apps`); else its package.
- **Occlusion.** Hidden and minimized windows are not presented into; a
  window that becomes visible shows the last frame at once.
- **Home.** An app that starts a HOME intent while its task is in front
  brings the desktop (the home task) to the front: its app hides, as Cmd+H
  hides a Mac app, and its tasks stay; the Dock icon brings it back
  (m1-shell.md, D6). The server, which shows several apps, minimizes that
  app's windows instead. A task Android moves to the back (Back on its
  root activity) is not a HOME intent and hides nothing.

## Back

The window chrome has no Back button (user's decision, #294). Back is the
keyboard's `KEY_BACK` (`Generic.kl`: BACK), or the mouse's back button:

- **Cmd+[**, the macOS Back shortcut. AppKit sends no key up while Command
  is held, so the key down presses and releases Back.
- **The mouse's back button** (button 4): the mouse's `BTN_SIDE`, which
  Android turns into Back ([input.md](input.md)).
- **A two-finger swipe to the right** of 80 points, mostly sideways, when
  the Mac's "Swipe between pages" is on: once per gesture. A gesture whose
  first 8 points go mostly right is the swipe and does not scroll the
  content horizontally (nor does its momentum); any other gesture scrolls.
- **Esc** goes out as Esc (`KEY_ESC`), for the apps and games that use it.
  Android 16 does **not** turn an unhandled Esc into Back: its
  `PhoneWindowManager`/`KeyGestureController` intercept an unhandled Esc to
  close system dialogs before the `Generic.kcm` fallback (ESCAPE → BACK)
  applies. Command is Android's Ctrl ([input.md](input.md)), so Cmd+Esc and
  Cmd+Left are not Android's Meta+Esc and Meta+Left (Back): Cmd+Left is
  Home, as in a Mac text field.

## Shortcuts

Command is the shortcut key and Android's Ctrl ([input.md](input.md)),
except for the window's own:

- **Cmd+W** closes the window, which removes its task.
- **Cmd+Q** quits the app: in a shim, the shim quits, closing the tasks
  it shows; in the server's own windows, every window of the key window's
  package closes.

The first window of a run shows a hint at its bottom ("Swipe with two
fingers or press ⌘[ to go back") until it is dismissed once; the dismissal
is kept in the `dev.aim` defaults (`BackHintDismissed`).

## App shims

Each launcher entry of the guest (a launcher activity, as a launcher lists
them: a package with two, like the Google app's "Google" and "Voice
Search", has two) is also a small macOS app, a **shim**, with the entry's
name and icon, so it has its own Dock icon, Cmd+Tab entry, Launchpad and
Spotlight presence (#248, #356):

```text
Settings.app/Contents/
  Info.plist            CFBundleName "Settings", AIMPackage com.android.settings,
                        AIMActivity com.android.settings.Settings,
                        AIMAppLabel "Settings", AIMPrimary,
                        AIMDisplaySocket <server socket>
  MacOS/aim-app         the aim-display binary (an APFS clone)
  Resources/AppIcon.icns
```

- **Hosting.** Run from a bundle that names a package, aim-display is that
  entry's **window host** (`bin/aim-display/shim.rs`). It connects to the
  display server (`OP_HOST`, naming its package and activity), which hands
  it the task records of the tasks started with its activity and, for the
  package's primary shim (the entry named as the app, else its first;
  else any running shim of the package), the package's other tasks, and
  the frames (each buffer's memfd once, then every present). It draws its
  windows' parts of each frame as the server does and answers when its GPU
  pass has read the buffer; the server's present waits for that (250 ms at
  most) and its present fence counts it. Its requests go to the task
  bridge and its input to the server's devices, through the server
  (`bin/aim-display/hosts.rs`). Tasks no host takes stay in the server's
  own windows; a host that connects takes its tasks' windows over, and one
  that quits or crashes gives them back.
- **Launch.** Opening a shim (Finder, Dock, Launchpad, Spotlight, `open`)
  starts its launcher activity in a new task (`LAUNCH`); Android brings a
  running task to the front instead, where its window is: the launch gives
  a task it started that has a window its own bounds as launch bounds
  (`ActivityOptions.setLaunchBounds`), or freeform would lay it out again
  as a new task and cascade it away from itself. Clicking the Dock icon
  again does the same. A shim with no window shows a splash at once, before
  it sets up its renderer or connects (#605): a window with the app's icon
  on its splash screen background, which moves into the task's window
  when Android gives the task bounds and goes when the launch has drawn
  its first frame (`DRAWN`, what `am start -W` waits for). Android draws
  no starting window of its own without WMShell (m1-shell.md, D4); with the
  window shell it does, and the splash goes once the task's window shows
  it ([task-organizer.md](task-organizer.md), "Starting windows"). The
  background is what Android's starting window takes (#606): the launcher
  activity's theme's `windowSplashScreenBackground`, else its
  `windowBackground` if a color, for the Mac's light and dark appearance,
  which aim-apps writes into the shim's `Info.plist`
  (`AIMSplashBackground`, `AIMSplashBackgroundDark`); else the Mac's
  window background.
  Quitting a shim closes the tasks it shows. Closing its last window
  does not quit it, as with a Mac app (an app may pass through a task that
  closes before its next one opens); a shim whose bundle is removed (its
  app uninstalled) quits once it has no window.
- **The server** has no Dock icon in window mode (an accessory app): each
  app is its shim. Its own windows show the tasks no shim shows (packages
  without a launcher activity, or whose shim is not running), titled as
  above.
- **Stacking.** With windows in several processes, the server restacks the
  tasks by the screen's order of all their windows (`CGWindowListCreate`)
  when one is minimized.
- **Icons** (`crates/aim-apps`, macOS icon grid of 1024): the body is an
  824 square with continuous corners (radius 185.4), 100 in from each edge,
  over a soft shadow. An adaptive icon's background and foreground are
  drawn on the 108 dp canvas with its 72 dp viewport filling the body, so
  the foreground's safe zone is never cut; a legacy icon sits at three
  quarters of the body on a white plate. The drawables are Android's own
  resources, read from the APK: PNG and WebP bitmaps, vector drawables
  (paths, groups, clip paths, strokes, gradients, tints), `inset`,
  `layer-list`, `shape`, `selector`, colors and theme attributes, and the
  level drawables at their level (0 unless a `scale`'s `android:level`
  sets one), as Android draws them: `rotate` at its angle for the level
  (the Clock's hands), `level-list`'s item for it, `scale` shrunk by it
  and `clip` cut to it (neither draws anything at level 0). The `.icns` has every size
  from 16 to 1024 (1x and 2x), drawn at its own resolution.
- **Install and uninstall.** `aim-apps shims --watch` keeps a directory
  holding one shim per launcher entry (an enabled MAIN/LAUNCHER activity or
  alias; its label and icon, else its target activity's, else the app's),
  from the image's app directories and `/data/app` (installed apps,
  updates of system apps and decompressed ones), rewriting a shim whose
  entry or icon drawing changed and removing the shims of uninstalled apps
  and disabled activities; it follows `/data/system/packages.list`.
  Written bundles are registered with Launch Services, and removed ones
  unregistered (`lsregister -u`). `cargo aim boot --windows` runs it into
  `target/aim/boot/apps` and removes them when the boot ends (`aim-apps
  clean`); `aim-apps install` writes into `~/Applications/aim Apps`
  instead (not run by the build or the tests).
  Each bundle is signed ad hoc (`codesign --sign -`), which macOS requires
  of an app that posts notifications. One more shim, "Android System"
  (`android`, framework-res's label and icon, no activity), stands for the
  platform.
- **Identity.** Launch Services, Notification Center, the Dock and
  Spotlight tell apps apart by bundle identifier. The shims of the user's
  own guest (`aimctl`'s default data directory) have stable identifiers,
  `dev.aim.app.<package>` for a package's primary shim and
  `dev.aim.app.<package>.<activity>` for its others, so their
  notification settings last. Every other guest's (`cargo aim boot
  --windows`, `aimctl --data`) are scoped to its data directory
  (`aim-apps shims --scoped`: `dev.aim.app-<hash>.<package>`, the hash of
  the path of its `/data`), so two window-mode guests on one Mac keep
  apart, and their entries go with their shims.
- **Notifications.** A shim shows its app's notifications as its own
  ([notifications.md](notifications.md)); the server opens it in the
  background (`--notifications`: no activity, no Dock icon until a window
  opens) when a notification comes and it is not running.

## Not covered yet

System windows that belong to no task (ANR and other system dialogs,
toasts, SystemUI's heads-up notifications) are drawn where Android puts them on the
display, which only a window over that place shows. Windows on a screen
other than the main one show nothing there (the display covers the main
screen only).

An activity start into a running task from anywhere but a shim (an app's
own `FLAG_ACTIVITY_NEW_TASK` start, a notification's `PendingIntent`,
`am start`) moves its window by freeform's cascade: freeform lays the
reused task out again from its persisted bounds, and moves it off itself
(#613). An activity of a fixed manifest orientation that comes to the top
of a running task does not turn the window: a freeform task is never the
display's orientation source, so `onTaskRequestedOrientationChanged` does
not come, and the top activity's `ActivityInfo` is only in the
`RunningTaskInfo` past its `Configuration`, with a whole `ApplicationInfo`
in it. The native task organizer's `onTaskInfoChanged` gives it (#593,
#463). Both are covered by the window shell, which makes
window mode desktop windowing (a fullscreen display, freeform tasks: no
persisted bounds to lay a task out from) and reports the top activity's
orientation to the bridge ([task-organizer.md](task-organizer.md)).
