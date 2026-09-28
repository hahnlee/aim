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
- **The caption and the bars are out of sight.** The freeform caption
  (WMShell's window decoration, 42 dp) lies inside the top of the task,
  under the window's title bar, which is made at least as tall (a unified
  toolbar). The status bar lies under the menu bar; the display has
  `BAR_MARGIN` rows below the screen, where the navigation bar or taskbar
  lies.

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

## The task bridge

`aim-windows` is a system daemon (`/system_ext/etc/init/aim-windows.rc`,
`user system`) that speaks the platform's own binder interfaces:
`IActivityTaskManager` (`registerTaskStackListener`, `getTaskBounds`,
`resizeTask`, `setFocusedTask`, `removeTask`, `startActivityAsUser`),
`IWindowManager` (`get`/`setWindowingMode`, base display size and density)
and the `ITaskStackListener` it registers. These are Java AIDL interfaces
with no NDK backend, so the few calls are written by hand with the
transaction codes of the pinned AIDL (first method = 1); the image's
`framework.jar` stubs have the same `TRANSACTION_*` values. The
`RunningTaskInfo` of the callbacks is read up to its `TaskDescription`
label, in the layout of the pinned `TaskInfo`, `Intent` (with
`prevent_intent_redirect`, which the image enables), `ComponentName`,
`Uri` and `Bundle`.

It connects to the display server with `FN_WINDOWS`, which answers the
server's mode:

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
| `FRONT` | guest | the task is the focused, top one |
| `MOVED_TO_BACK` | guest | Back on its root activity moved it behind the others |
| `REMOVED` | guest | the task is gone |
| `SET_BOUNDS` | server | move or resize the task (`resizeTask`); the bridge answers with `TASK` |
| `FOCUS` | server | `setFocusedTask` |
| `CLOSE` | server | `removeTask` |
| `LAUNCH` | server | start a package's launcher activity in a new task |

## Windows

- **Placement.** A new task's window takes the bounds Android chose
  (freeform's cascade). When the screen does not allow them (above the
  menu bar), the task follows the window. Bounds Android gives a task
  itself (its launch position; a shift away from another task when an
  activity starts in it) do not always reach the task's surface: the
  legacy freeform transitions can leave it where it was, and the window
  would show another part of the display. The bridge commits them with
  `resizeTask`, whose change transition places the surface.
- **Move and resize.** Moving the window moves the task at once; a live
  resize resizes it when the resize ends (each resize is a configuration
  change for the app). The window then takes the bounds the task got (a
  minimum size). While they differ, the layer shows the task one pixel per
  pixel from the top left, unstretched.
- **Focus and stacking.** The key window's task is the focused one
  (`FOCUS`), and a task Android brings to the front makes its window key
  while the app is active. A press in a window whose task is not in front
  focuses the task first and holds the touch until Android reports it in
  front (250 ms at most), so it lands on that task.
- **Minimize and close.** A minimized window's task goes behind the
  visible windows' tasks (they are focused back to front, in the screen's
  order). Closing a window removes its task; a removed task closes its
  window. Zoom (the green button) resizes; full screen is off.
- **Title.** The task's `TaskDescription` label, else its package.
- **Occlusion.** Hidden and minimized windows are not presented into; a
  window that becomes visible shows the last frame at once.

## Back

The window chrome has no Back button (user's decision, #294). Back is the
keyboard's `KEY_BACK` (`Generic.kl`: BACK):

- **Cmd+[**, the macOS Back shortcut. AppKit sends no key up while Command
  is held, so the key down presses and releases Back.
- **The mouse's back button** (button 4).
- **A two-finger swipe to the right** of 80 points, mostly sideways, when
  the Mac's "Swipe between pages" is on: once per gesture.
- **Esc** goes out as Esc (`KEY_ESC`), for the apps and games that use it.
  Android 16 does **not** turn an unhandled Esc into Back: its
  `PhoneWindowManager`/`KeyGestureController` intercept an unhandled Esc to
  close system dialogs before the `Generic.kcm` fallback (ESCAPE → BACK)
  applies. Android's own Meta+Esc and Meta+Left are Back, so Cmd+Esc and
  Cmd+Left are too.

The first window of a run shows a hint at its bottom ("Swipe with two
fingers or press ⌘[ to go back") until it is dismissed once; the dismissal
is kept in the `dev.aim` defaults (`BackHintDismissed`).

## Not covered yet

System windows that belong to no task (ANR and other system dialogs,
toasts, heads-up notifications) are drawn where Android puts them on the
display, which only a window over that place shows. Windows on a screen
other than the main one show nothing there (the display covers the main
screen only).
