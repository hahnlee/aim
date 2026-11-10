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
 │ menu bar                   │             │                            │
 │   ┌─title bar─┐            │   s·(x,y)   │                            │
 │   │ Settings │ ┌──────┐    │ ──────────▶ │   ┌─task 9──┐ ┌─task 11─┐   │
 │   │ content  │ │Chrome│    │             │   │ content │ │ content │   │
 │   └──────────┘ └──────┘    │             │   └─────────┘ └─────────┘   │
 └────────────────────────────┘             └────────────────────────────┘
```

## Design: the display is the screen

The original freeform windowing gives every task bounds of its own on the
default display. Window mode makes that display **congruent with the Mac's
main screen**: display pixel (x, y) lies under screen point
(x, H·s − y) / s for a screen H points tall at s pixels per point, and each
task's window lies exactly over the task. The rest follows from that:

- **Each window is composed from its task's layers.** The composer HAL
  keeps the layers for device composition, and each window draws its own
  task's layers, one pixel per pixel, at their place on the display
  ([layers.md](layers.md)): where two tasks overlap, neither window shows
  the other's pixels. Layers of no task (the IME, system dialogs) show in
  system panels at their place on the screen; an app's toast lies within
  its task and shows in its window.
- **Input stays one touchscreen.** A press in a window is a touch at the
  display pixel under it, which Android's own hit testing sends to the
  task (and its dialogs and popups) there.
- **Android pixels and macOS points stay apart.** The display is the
  screen in backing pixels, never an upscale; `aim_host_display::windows`
  is the one place that converts.
- **No caption or system bars.** The lightweight shell draws neither, so
  the window's standard Mac title bar sits above the task and the display
  has exactly the screen's backing pixels (#681).

## Pieces

| Piece | Where |
| --- | --- |
| Geometry (screen points ↔ display pixels, content, caption) | `crates/aim-host-display/src/windows.rs` |
| Task windows (AppKit), back hint | `crates/aim-host-display/src/bin/aim-display/windows.rs` |
| Composing each window from its task's layers | `bin/aim-display/metal.rs` (`Target`, `LayerFrame`) |
| Which task each layer is (attribution) | `crates/aim-host-display/src/layers.rs` |
| System panels (layers of no task) | `bin/aim-display/panels.rs` |
| Task bridge (guest, Rust): task listener and task calls | `daemons/windows` (`/system_ext/bin/aim-windows`) |
| Task bridge of the app host image (host, Rust): its own tasks | `crates/aim-services/src/app_host/tasks.rs` |
| `RunningTaskInfo` parcel reading | `daemons/windows/core` |
| `RunningTaskInfo` of the app host's tasks (`getTasks`) | `crates/aim-services/src/app_host/task_info.rs` |
| Host-call `FN_WINDOWS`, records | `aim_hostcall::display::{Windows, Window, window}` |
| Freeform support | `/vendor/etc/permissions/android.software.freeform_window_management.xml` |
| Window hosts (the server's side; a shim's side) | `bin/aim-display/hosts.rs`, `bin/aim-display/shim.rs` |
| APK labels and icons, macOS icons, shim bundles | `crates/aim-apps` (`aim-apps`) |

## The task bridge

`aim-windows` is a system daemon (`/system_ext/etc/init/aim-windows.rc`,
`user system`) that speaks the platform's own binder interfaces:
`IActivityTaskManager` (`registerTaskStackListener`, `getTaskBounds`,
`resizeTask`, `setFocusedTask`, `removeTask`, `startActivityAndWait`),
`IWindowManager` (base display size)
and the `ITaskStackListener` it registers (tasks, focus and
`onActivityRequestedOrientationChanged`). These are Java AIDL interfaces
with no NDK backend, so the few calls are written by hand with the
transaction codes of the pinned AIDL (first method = 1); the image's
`framework.jar` stubs have the same `TRANSACTION_*` values. The
`RunningTaskInfo` of the callbacks is read up to its `TaskDescription`
label and primary/background colors, in the layout of the pinned
`TaskInfo`, `Intent` (with `prevent_intent_redirect`, which the image
enables), `ComponentName`, `Uri` and `Bundle`.

It connects to the display server with `FN_WINDOWS`, which answers the
server's mode. Its binders die with system_server, so its rc starts it
again when zygote restarts (`init.svc.zygote=restarting`); the server
drops the old connection's tasks. The mode:

- **Device mode:** it exits. DeviceServices keeps the default display area
  fullscreen in both modes, including the migration from old freeform data.
- **Window mode:** it attaches to the window shell and reports tasks. A task
  gets a window once an activity of it ran and it has bounds of its own on
  the display. The shell's activity type identifies the home task; tasks
  restored from recents but not started get no window.

| Record | From | Meaning |
| --- | --- | --- |
| `TASK` | guest | the task's bounds and windowing mode, new or changed (`aim-windows` sends no mode: its windows never take the full screen) |
| `PACKAGE`, `TITLE` | guest | the task's package; its `TaskDescription` label |
| `COLORS` | guest | the task description's ARGB primary and background colors (zero means unset) |
| `ACTIVITY` | guest | the activity the task was started with, `package/class` (`origActivity`, an alias, else `realActivity`) |
| `FRONT` | guest | the task is the focused, top one |
| `MOVED_TO_BACK` | guest | Back on its root activity moved it behind the others |
| `ORIENTATION` | guest | an activity of it asked for landscape, portrait or neither (`setRequestedOrientation`) |
| `REMOVED` | guest | the task is gone |
| `HIDE` | guest | the desktop (the home task) came to the front over the task: its app hides |
| `DRAWN` | guest | the `LAUNCH` of `package/class` drew its first frame, or ended (its task's window shows its starting window) |
| `PINNED`, `UNPINNED` | guest | the task entered or left picture-in-picture: its window floats, or no longer ([task-organizer.md](task-organizer.md), "Picture-in-picture") |
| `SET_BOUNDS` | server | move or resize the task (`resizeTask`); the bridge answers with `TASK` |
| `FOCUS` | server | `setFocusedTask` |
| `CLOSE` | server | `removeTask` |
| `LAUNCH` | server | start a package's launcher activity in a new task (`startActivityAndWait`, in a thread of its own; `DRAWN` when it returns) |
| `SET_WINDOWING_MODE` | server | the user put the window into or out of the Mac's full screen: fullscreen or freeform (app host only); the bridge answers with `TASK`, also when it refuses |

## The app host

On the app host image ([app-host.md](app-host.md)) there is no
system_server and no `aim-windows`: the app host, whose tasks they are,
is the bridge. Its tasks are the original's (`ActivityStarter`): an app
has as many as its starts make (a `singleInstance` activity, a NEW_TASK
of another affinity, MULTIPLE_TASK, a start from a service), and the
front one with a running activity is focused. Without window mode each
task has the whole display and hides those behind it, whose activities
pause and stop, unless none of its activities occludes it (their themes
translucent or floating): a task behind it stays visible, paused
(`TaskFragment.getVisibility`'s visible behind translucent). Within a
task, an activity hides those under it unless it is translucent. Apps see
the tasks as `getTasks` lists them (`RunningTaskInfo`, its bounds and
visibility included). At `systemReady` it starts the home activity the
package manager resolves for `ACTION_MAIN`/`CATEGORY_HOME` in the root
home task, with the HOME activity type (`startHomeOnAllDisplays`,
`app_host/home.rs`), and again once that task has gone and no task is
left to focus (`resumeHomeActivity`): on fresh data the setup wizard,
which provisions the device and finishes, then the image's
`dev.aim.home/.Home`. Without window mode that task is an ordinary task,
made on top (`getOrCreateRootHomeTask(ON_TOP)`): the home is resumed until
an app covers it. A home record is
unresizeable unless its activity asked to be resizeable
(`setActivityType`). In window mode the Mac desktop is the home: the root home task
stays behind it at the bottom of the tasks, invisible and never focused,
its activity launched straight to stopped (`LaunchActivityItem` with
`StopActivityItem`), and it is never reported to the display server, so it
has no Mac window. A start that brings it to the front (a HOME intent,
`moveTaskToFront`; an app's HOME intent also brings a root home task it
makes or finds empty, where `startHomeOnAllDisplays` and
`resumeHomeActivity` leave it behind) brings the desktop there: the tasks
of the focused task's app go behind the desktop, invisible, and pause and
stop; the server hides that app (`HIDE`); the home becomes the focused task
and its activity the top resumed one; other apps' tasks stay visible and
resumed, as their windows stay. A task of the hidden app coming to the front (its
window back from the Dock) brings all of the app's tasks back, as
unhiding shows all its windows; any other task coming to the front puts
the home behind the desktop again, paused and stopped. When the home's
process dies, a home that saved its instance state at its stop stays in
the task (`handleAppDied`); behind the desktop it should not be visible, so
it is not restarted (`attachApplication` starts only activities that
should be), and stays unlaunched until a start brings the task to the
front, which relaunches it from that state. `getTasks`, the task
organizer, the window dump and the root-task queries list it.
`getAllRootTaskInfos` and `getAllRootTaskInfosOnDisplay` list the root
tasks top to bottom (the app tasks, the organizer root tasks with their
tasks as leaves, and the root home task: tasks and the root home task are
made on top, organizer root tasks at the bottom, under the root home task
too; in window mode the root home task is made at the bottom instead,
behind the desktop, until a HOME start brings it to the front), and
`getRootTaskInfo` and
`getRootTaskInfoOnDisplay` find the root home task for
`ACTIVITY_TYPE_HOME`, no pinned one, else the topmost compatible one
(`TaskDisplayArea.getRootTask`), and `getFocusedRootTaskInfo` the focused
task's root task, all behind `MANAGE_ACTIVITY_TASKS`
(`app_host/root_tasks.rs`). A start whose record is a home
activity (`ActivityRecord.setActivityType`) goes to that task, and no
other start is put there (`FindTaskResult` compares activity types). It connects to the display server
itself (`OP_WINDOWS`, from guest-init) and, in window mode:

- **Opens a task** for each task it makes, at the bounds freeform
  gives a task started without any (`TaskLaunchParamsModifier
  .getTaskBounds`: 412×732 dp, 1064×600 dp for a landscape activity, fit
  into the display and centred, or 75 dp down and right of a freeform
  source task of that size; launch bounds turned to the activity's
  orientation; an activity's `<layout>` size and gravity, which a start
  into its running task applies again (`getLayoutBounds`); then moved off
  another freeform task's corners), and
  reports it (`TASK`, `PACKAGE`, `ACTIVITY`, then `FRONT`). Its
  activities' configuration is the freeform task's (`TaskFragment
  .computeConfigResourceOverrides`: its sizes from its bounds). Every
  task's top activity stays resumed; the focused one's is the top resumed
  one, and a task Android brings to the front, or that becomes focused as
  the one in front of it finishes its last activity, is reported (`FRONT`).
  A task goes (`REMOVED`) with its last activity.
- **Places it**: the task's layer is a container at the display's root,
  the parent of its windows' layers, moved to the task's bounds and cut to
  them by a SurfaceFlinger transaction, stacked in the tasks' order.
- **Takes requests**: `LAUNCH` brings the task a launcher's start would
  find (`findTask`: the one started with the activity, else of its
  affinity) to the front, or starts the activity in a new task and answers
  `DRAWN` when it is idle; `FOCUS` brings the task to the front, its
  windows in front of the others and its latest one with the input focus;
  `SET_BOUNDS` (`resizeTask`) gives the task the bounds as its windowing
  mode resolves them (`Task.resolveLeafTaskOnlyOverrideConfigs`, undefined
  inheriting its parent's): fullscreen fills its parent, pinned keeps
  them, the others take them at least its minimal size
  (`adjustForMinimalTaskDimensions`: 220 dp, or its activity's window
  layout's). Bounds Android gives a freeform task (launch bounds, a WCT's
  `setBounds`) then keep 48x32 dp of them and their top on its parent's
  part of the display (`computeFreeformBounds`; no bars, so the stable
  area is the display), but a `SET_BOUNDS` is the Mac window's own frame
  and keeps its place (see Move and resize); the Mac window follows the
  bounds the task got. It tells its activity as
  `ActivityRecord.updateReportedConfigurationAndSend` does: a change the
  activity does not handle (`configChanges`), past its size buckets
  (`SizeConfigurationBuckets.filterDiff`), relaunches it, keeping its
  window when only sizes changed; anything else is a configuration change
  (`ActivityConfigurationChangeItem`) and its windows' new frames
  (`WindowStateResizeItem`). An app's process follows the configuration
  of the activity it launched last and still has, as
  `WindowProcessController` registers to it (a system-uid process keeps
  the global one): its Application resources take that task window's
  size, at launch (`LaunchActivityItem`) and in a `ConfigurationChangeItem`
  sent before the activity's when the task resizes, the global
  configuration changes or that activity is destroyed. `CLOSE` finishes
  the task's activities and kills its package's processes, as `removeTask` does
  (`killTaskProcessesIfPossible`), unless one has an activity in another
  task: closing Clock's alarm window leaves Clock's own window.
- **Input**: the agent sends a touch to the front window under it, in the
  window's coordinates (`aim-app-host-agent`).

### Display rotation policy (#2630)

The Mac display stays at its natural rotation, `ROTATION_0`. The native
window owner accepts rotation preferences as Android policy: `wm user-rotation
[free|lock] [rotation]`, `fixed-to-user-rotation`, and
`set-ignore-orientation-request` / `get-ignore-orientation-request` use the
pinned Android 16 shell's output, including the strings CTS saves and parses.
An explicit lock stores the requested rotation (0–3); a lock without a value
captures the effective rotation, 0. Neither changes scanout or display metrics.

Persistence follows `DisplayRotation` and `DisplayWindowSettings`: the default
display's user lock and rotation are `accelerometer_rotation` and `user_rotation`
in Settings.System; fixed mode and the nullable ignore override live in
`/data/system/display_settings.xml`. Resetting the ignore override reveals the
product/vendor default or Android's large-screen default. XML writes preserve
other display settings and rows. Binder rotation operations share this owner,
including SET_ORIENTATION checks; queries and the original local ignore-reset
operation need no such permission.

App orientation requests remain activity policy. Under the original freeform
rules (`TaskFragment.computeConfigResourceOverrides`), a task's configuration
comes from its bounds, and a freeform task is not the display's orientation
source. With display rotation fixed at 0, recording a rotation preference
therefore neither swaps a task's bounds nor rotates its Mac window. The legacy
bridge's orientation-to-window resizing described below is separate from this
native app-host display policy. Remaining shell owners, including overrides
and letterboxing, are tracked in #2050.

## Windows

- **Placement.** A new task's window takes the bounds Android chose
  (freeform's cascade). When the screen does not allow them (above the
  menu bar), the task follows the window. The window shell's transitions
  place every task's surface ([task-organizer.md](task-organizer.md)).
- **Where a window reopens.** A launcher activity's new task opens where
  its window last was, as a Mac app's window does (#635): the bridge keeps
  each activity's last bounds in `/data/system/aim-windows-places` and
  gives them to a `LAUNCH` as launch bounds while they lie within the
  display. Starts from elsewhere (an app's own new task, a notification)
  open at Android's default bounds, as on a desktop-windowing device.
- **Move and resize.** Moving the window moves the task at once; a live
  resize resizes it when the resize ends (each resize is a configuration
  change for the app). The window then takes the bounds the task got (a
  minimum size), once the user has not moved it for half a second; bounds
  Android gives the task itself it takes at once. While they differ, the
  layer shows the task one pixel per pixel from the top left, unstretched.
  A window the user drags partly off the screen or onto another monitor
  stays where the user put it, as a Mac window does: the task takes the
  window's bounds unfitted, so its configuration matches the window, and
  the part of it outside the display is simply not visible to Android.
- **Focus and stacking.** The key window's task is the focused one
  (`FOCUS`), and a task Android brings to the front makes its window key
  while the app is active; while inactive it orders the window directly
  above the frontmost AIM task window, including cooperating hosts, using
  the window numbers they report. Windows behind another Mac app stay
  behind it, without activation or a change of key window; minimized
  windows stay minimized. A press in a window whose task is not in front
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
  window. Zoom (Option and the green button) resizes.
- **Full screen.** A fullscreen task's window (`setWindowingMode`, a
  fullscreen launch or saved launch params) is in the Mac's full screen, a
  space of its own, with AppKit's animation; the display is the main
  screen, so its content there is the task's display-sized bounds, one
  pixel per pixel. The window enters from its freeform frame (a fullscreen
  launch's from where a new freeform task opens), never resized to the
  display first, and AppKit animates it back there. A task leaving
  fullscreen takes the window out, and it then takes the task's freeform
  bounds (requested, else last non-fullscreen; for the user's leaving with
  neither, a new freeform task's default bounds).
  The user's full screen (the green button, Ctrl-Cmd-F) asks the app host
  for fullscreen or freeform (`SET_WINDOWING_MODE`, as `setWindowingMode`
  does), and the window waits for the task's answer: a refusal (a task in
  an organizer root task) takes it back. While it moves into or out of
  the full screen, its frames do not move or resize the task. AppKit does
  not minimize a window in the full screen: a fullscreen task that goes
  home or to the back takes its window out first (the task stays
  fullscreen), and the window minimizes once out; back from the Dock, it
  enters the full screen again.
- **Split screen.** The tasks of an organizer's adjacent root tasks
  (`setAdjacentRoots`) keep a Mac window each, at their root task's bounds
  even where a task's minimal size grows it past them (its windows then
  take touches only within the root task's bounds), so the two windows
  tile side by side. A task that requests bounds of its
  own there (a launch's bounds, as `LaunchParamsController.layoutTask` sets
  them on a task in a multi-window root task) takes them in its
  configuration, but its window stays a tile. The user's drag of one moves both;
  a resize moves its root task's edges, and the adjacent root's edges that
  met them follow, as the split divider moves both stages; bounds that would
  leave the other empty are refused and the window goes back. Once adjacency is cleared
  each window moves on its own.
- **Multi-window root tasks.** A task the display area holds in
  multi-window mode (a multi-window launch, or `setWindowingMode`, with no
  organizer root task to go in) keeps its own Mac window at its bounds, the
  display area's unless it requests others, like a split-screen tile
  without a partner: the user's resize of the window resizes the task
  (`canResizeTask`), and the window has no full screen, which only
  fullscreen and freeform tasks map to.
- **Title.** The task's `TaskDescription` label, else what a launcher
  calls it: in a shim, the shim's name; in the server, the label of the
  launcher activity the task was started with, else its app's label (both
  from the shims in `--apps`); else its package.
- **Title-bar color.** The task description's primary color, else its
  background color, in sRGB through AppKit's transparent title bar and
  window background; both unset restores the standard macOS chrome.
  Updates and a handoff between the server and a shim retain the colors.
- **Occlusion.** Hidden and minimized windows are not presented into; a
  window that becomes visible shows the last frame at once.
- **Home.** An app that starts a HOME intent while its task is in front
  brings the desktop (the home task) to the front: its app hides, as Cmd+H
  hides a Mac app, and its tasks stay; the Dock icon brings it back
  (m1-shell.md, D6). The server, which shows several apps, minimizes that
  app's windows instead, and one of them back from the Dock brings back
  the others it minimized, as the guest brings back the app's tasks. A task Android moves to the back (Back on its
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

Windows on a screen other than the main one show nothing there (the
display covers the main screen only).

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
