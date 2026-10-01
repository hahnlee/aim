# The window shell: task organizer and transition player (#463, step 8)

In window mode ([windows.md](windows.md)) without the Android shell
([m1-shell.md](m1-shell.md)), nothing registers WMShell's two roles: the
task organizer and the transition player. This document is the design of
the device's own, a "Mac-specific WMShell" (decisions D3, D4 and D5 of
#463): a task organizer and a transition player in Java, in system_server,
that make window mode Android 16's desktop windowing and tell aim-windows
what only an organizer learns. It is built into every image and active only
in the lightweight shell (`ro.vendor.aim.lightweight_shell`), a check-only
variant until the switch.

Evidence is from the pinned sources (`android-16.0.0_r1`,
`platform/frameworks/base`; `wm/` is
`services/core/java/com/android/server/wm/`, `Shell/` is
`libs/WindowManager/Shell/src/com/android/wm/shell/`).

## Summary

- **#613 is not a placement bug an organizer can fix.** A start into a
  running freeform task lays it out again from the bounds
  `LaunchParamsPersister` recorded for it, and moves it off itself
  (section 2). The persister records only on a *freeform* display area,
  which is what aim-windows makes the default display today. Android 16's
  desktop windowing keeps the display area fullscreen and the tasks
  freeform; there the persister records nothing and a running task keeps
  its bounds for every caller.
- **A fullscreen display area needs a transition player.** A task started
  without a freeform source or options is fullscreen; WMShell's
  `DesktopTasksController.handleRequest` turns it freeform in the
  transition's request, before it is shown. An organizer alone hears of a
  task only once it has been visible (`Task.taskAppearedReady`), too late.
- **Registering a player turns shell transitions on**, the configuration
  every Android 13+ device runs in. The player applies each transition at
  once, as WM itself does when a player fails
  (`Transition.cleanUpOnFailure`), and WM's finish transaction places and
  crops every task that took part: nothing is animated, and no leash is
  placed by hand.
- **The organizer** receives every root task and its `TaskInfo`: the top
  activity's manifest orientation (#593) and the task's activity type,
  which it reports to aim-windows over binder (`aim.window_shell`).
  aim-windows keeps its host-call connection and its `IActivityTaskManager`
  calls, and drops the "a pixel and back" commit.
- **Starting windows** are the organizer's to draw, and it draws them:
  the activity's splash screen, which apps can take over with the
  SplashScreen API (section 5).
- **Picture-in-picture** is the shell's too: it gives a task entering PiP
  its bounds and mode, as WMShell does, and the task's Mac window floats
  (section 6).
- **Later** the organizer becomes aim-windows' task source (one Mac window
  per organized task).

## 1. What registering changes

### The organizer

There is one organizer for all root tasks
(`wm/TaskOrganizerController.java:569`, the last registered); registering
hands it the existing tasks with a leash each (`:497-537`). For an
organized task WM stops:

| What | Where | Without WMShell's organizer |
| --- | --- | --- |
| Positioning the task's surface | `wm/WindowContainer.java:3211-3215` | WM positions it on every layout |
| Cropping it to its bounds | `wm/Task.java:2334-2336` | WM crops root tasks |
| Its own resize handles for freeform tasks | `wm/DisplayContent.java:5248` | unused: the Mac resizes |
| Animating it | `wm/Task.java:3032` | no animations either way |
| Drawing no starting window | `wm/TaskOrganizerController.java:623-641`, `wm/StartingSurfaceController.java:83-92` | with an organizer, WM asks it for one (below) |

What WM keeps: the task's visibility (`Task.prepareSurfaces`,
`wm/Task.java:3288-3335`, for every task the organizer did not create), its
z-order, and input, whose crop then follows the organized surfaces
(`wm/InputMonitor.java:300`). WMShell's freeform listener does nothing to
a leash with shell transitions on (`Shell/freeform/FreeformTaskListener.java`,
`onTaskAppeared`), since the transitions place it.

So registering an organizer without a player would make us place every
leash on every bounds change, from `onTaskInfoChanged`, which WM defers
until a task has been visible (`wm/Task.java:4341-4355`) and batches per
layout (`wm/TaskOrganizerController.java:257-290`): a moved task would
show its old place until the callback. With the player, the transitions do
it.

Starting windows: `addStartingWindow` returns true once any organizer is
registered, and WM records a starting surface that only the organizer can
give a window (`wm/TaskOrganizerController.java:623-647`). Without one,
nothing waits for it, but the splash transfer an app asks for with
`SplashScreen.setOnExitAnimationListener` needs the starting window
(`wm/ActivityRecord.java:2533-2556`): the app's exit listener is not
called, and a transition becomes ready on the app's own first frame. So the
organizer draws them (section 5).

### The player

`isShellTransitionsEnabled()` means "a player is registered"
(`wm/TransitionController.java:384-386`). With one, every visible change
is a transition: WM collects it, asks the player to start it
(`requestStartTransition`, `:858`), waits until the participants have drawn
(BLAST sync), then hands the player a start and a finish transaction
(`onTransitionReady`, `wm/Transition.java:1987-2025`). The player must:

1. answer every request with `startTransition(token, wct)`, the WCT being
   its policy for the change (null for none);
2. apply the start transaction, play, apply the finish transaction and
   call `finishTransition` for every ready transition. The finish
   transaction reparents, relayers, and resets position, crop, alpha and
   matrix of every participant to its final state
   (`buildFinishTransaction`, `:1126-1160`; `resetSurfaceTransform`,
   `:1095-1118`), organized tasks included.

WM's own fallback for a player that failed is exactly step 2 without
playing (`cleanUpOnFailure`, `:2273-2284`), and it is what WM does with a
transition when no player is enabled (`:2016-2025`). The player here does
that: no animation, since the Mac animates windows.

Shell transitions change more than surfaces: `resizeTask` becomes a
transition (`wm/ActivityTaskManagerService.java:2969-2990`), queued behind
a running one and ready once the app redrew; visibility is committed when
the transition is ready, not at once (`wm/ActivityRecord.java:5490`). This
is how every device behaves, so the CTS expectations get closer, and
aim-windows' "a pixel and back" commit, which exists because the legacy
path could leave a surface behind (windows.md, "Placement"), goes.

## 2. #613: window mode as desktop windowing

The chain for `am start` (or an app's `FLAG_ACTIVITY_NEW_TASK` start, or a
notification's `PendingIntent`) into a running freeform task:

1. `ActivityStarter.setTargetRootTaskIfNeeded` lays the reused task out
   again (`fix_layout_existing_task`, enabled in the image;
   `wm/ActivityStarter.java:3050-3054`).
2. `LaunchParamsController.calculate` starts from the persister's record
   for the task's component (`wm/LaunchParamsController.java:89-91`).
3. `TaskLaunchParamsModifier` takes those bounds as fully resolved
   (`wm/TaskLaunchParamsModifier.java:250-273`) and still keeps them from
   "stomping on an existing task" (`:377-386`); the tasks it checks
   include the task itself (`:845-858`), so its own bounds conflict and
   the cascade step moves it.
4. `layoutTask` sets the moved bounds on a multi-window root task
   (`wm/LaunchParamsController.java:130-158`).

Launch bounds in the options skip step 3, which is why aim-windows'
`LAUNCH` passes a running task's own bounds; every other caller moves it.
WMShell leaves it alone because desktop windowing never gets there: its
display area stays fullscreen, and the persister records a task only on a
freeform display area (`wm/Task.java:2387-2393`). With no record, a running
task with bounds of its own is left as it is
(`wm/TaskLaunchParamsModifier.java:387`), and `layoutTask` returns without a
change.

Moving the task back afterwards (from the organizer's `onTaskInfoChanged`
or the player's request) would be a second layout the app sees, and
#613 rules it out. So window mode becomes desktop windowing:

| | Freeform display area (now) | Desktop windowing (the window shell) |
| --- | --- | --- |
| The default display's windowing mode | freeform (aim-windows' `setWindowingMode`) | fullscreen |
| A task's windowing mode | inherited: freeform | freeform, set per task |
| A start into a running task | laid out from the persisted record, cascaded (#613) | keeps its bounds |
| A new task from a freeform task | freeform, cascaded from the source | the same: freeform is inherited from the source (`wm/TaskLaunchParamsModifier.java:586-609`) |
| A shim's `LAUNCH` | freeform, default bounds | freeform, default bounds: aim-windows asks for freeform (`ActivityOptions.setLaunchWindowingMode`), as a desktop's launcher does |
| A new task without a freeform source (`am start`, a notification) | freeform, default bounds | fullscreen, with the default freeform bounds as its restore bounds (`:192`, `wm/LaunchParamsController.java:151-153`); the player's WCT makes it freeform in its transition request, and the task takes those bounds (`wm/Task.java:2010-2029`), as `Shell/desktopmode/DesktopTasksController.kt:2439` does |
| The home task | fullscreen | fullscreen |
| Where a component's window opens next time | the persisted bounds | a shim's launch: where its window last was (aim-windows' record, #635); other starts: the default bounds |

Desktop mode itself (`config_isDesktopModeSupported`) stays off: its launch
params modifier and app-compat policies assume WMShell's desktop UI, and
the player covers the one case it would.

### Explicit windowing modes

A start that asks for a windowing mode (`ActivityOptions.setLaunchWindowingMode`,
as WM CTS does with fullscreen for most of its tests) keeps it on a freeform
display area. On a fullscreen one `TaskLaunchParamsModifier` resolves an
explicit fullscreen launch and one without a mode alike (`outParams.mWindowingMode`
is undefined for both, `wm/TaskLaunchParamsModifier.java:309-311`), and
neither `TaskInfo` nor `TransitionRequestInfo` carries the options, so the
player cannot tell them apart. The shell therefore watches the starts:
`LaunchModes`, an `ActivityInterceptorCallback` that never intercepts,
notes the mode each start of a component asked for, in
`ActivityStarter.executeRequest` (`wm/ActivityStarter.java:1350`) before the start requests its transition;
the player takes that note when the task opens and makes the task freeform
only when the start asked for none. A task that kept its mode keeps it when
it comes to the front again. Notes not taken (a start that reused a task
of another component) expire after 10 s. WindowManager allows one callback
per id and a product has one id, so one product interceptor dispatches to
this and to the notification-permission redirect (ADR 0013).

A data directory that booted with a freeform display area keeps the
persister's records (`/data/system_ce/<user>/launch_params`), which are
still read and give step 3 its bounds; the variant uses fresh data
directories (build.md, "Image variants"), and the switch must drop them
once (#636).

## 3. The component

`dev.aim.server.WindowShell`, in `aim-services.jar`
(`java/device-services`): the device-specific system service
(`DeviceServices`, docs/system-services.md, "The system_server bridge")
creates it when `ro.vendor.aim.lightweight_shell` is true, and registers the
organizer and the player at its first boot phase
(`PHASE_DEVICE_SPECIFIC_SERVICES_READY`), before the home activity starts.

**Why system_server.** Both registrations need `MANAGE_ACTIVITY_TASKS`,
`signature|recents` (`core/res/AndroidManifest.xml:4327`): SystemUI has it
by the platform key, which a package of ours cannot have. aim-windows has
the system uid but no `SurfaceControl` (the NDK has no
`SurfaceControl.Transaction` parcel, D5). system_server is where the device
services already run, and it holds the permission.

**In-process consequences.** WM calls a local binder directly, on its own
thread and under its global lock, and passes its own objects:

- Every callback is handed to the shell's thread (`AimWindowShell`, a
  `HandlerThread`; `TaskOrganizer`'s executor): an organizer or player that
  calls back into WM from inside the callback would re-enter a
  half-finished transition.
- WM closes a transition's start and finish transactions when it finishes
  (`wm/Transition.java:1291-1296`, the copies it keeps in case the player
  fails): the player merges them into transactions of its own inside the
  callback, then applies those on its thread.
- The `TransitionInfo` leashes and a task's `RunningTaskInfo` are WM's
  objects, not parcelled copies: the shell reads them and releases only the
  leash `onTaskAppeared` handed it (a copy made for the organizer,
  `wm/TaskOrganizerController.java:112-114`).
- An exception in the shell's code is an exception in system_server, on
  the shell's thread: each request is still answered and each transition
  still finished (`finally`), then it fails as any bug in system_server
  does.

**What it does (phase 2, placement):**

| Callback | Answer |
| --- | --- |
| `requestStartTransition` | `startTransition` with, in window mode, a WCT that makes a fullscreen standard task on the default display freeform |
| `onTransitionReady` | apply the start and finish transactions, `finishTransition`; in window mode, a fullscreen standard task that opened in it without a request of its own (a start while another transition collected, which it joins) is made freeform by a transition of the shell's (`startNewTransition`), which the app sees as a second layout |
| `onTransitionReady`, after `finishTransition` | tell aim-windows of each task that took part (`onTaskPlaced`): its surface is at its bounds now, showing what it drew |
| `onTaskAppeared`, `onTaskInfoChanged` | report the task's activity type and its top activity's `screenOrientation` to aim-windows when either changed |
| `onTaskVanished` | forget the task, release its leash |
| `addStartingWindow`, `removeStartingWindow`, `copySplashScreenView`, `onAppSplashScreenViewRemoved` | the task's splash screen window (section 5) |
| back on root | the base class's no-op |

**Later.** Each is a step of its own, with the CTS modules of section 9:

- *One Mac window per organized task*: `onTaskAppeared`/`Vanished`/
  `InfoChanged` replace aim-windows' `ITaskStackListener` and its partial
  `RunningTaskInfo` parsing (the bounds, title, package and focus the
  bridge reads by hand today).
- *Window drags*: a `resizeTask` per move is a transition that waits for a
  redraw; WMShell moves the leash during a drag and commits the bounds
  once (`Shell/windowdecor/DragPositioningCallbackUtility.java`). The organizer can
  do the same for the Mac's drags.

## 4. Talking to aim-windows and aim-display

The shell has no host-call connection: aim-windows keeps it and stays the
only guest side of the display server's window records (`FN_WINDOWS`).
The shell publishes one binder, `aim.window_shell`
(`dev.aim.server.IWindowShell`, `java/device-services/aidl`), which
answers only the system uid:

```text
 WM ──ITaskOrganizer, ITransitionPlayer──▶ WindowShell (system_server)
                                              │ IWindowShellListener
                                              ▼
 aim-display ◀──FN_WINDOWS records── aim-windows ──IActivityTaskManager──▶ WM
```

- `IWindowShell.attach(IWindowShellListener)`: aim-windows attaches in
  window mode. While a listener is attached, the player makes new tasks
  freeform; in device mode nothing attaches and tasks stay fullscreen. The
  listener hears every organized task at once and then each change
  (`onTaskChanged(taskId, activityType, orientation)`), and each task that
  took part in a finished transition (`onTaskPlaced(taskId)`); a dead
  listener is dropped (`linkToDeath`).
- aim-windows, when `ro.vendor.aim.lightweight_shell` is true: waits for
  `aim.window_shell`, sets the default display fullscreen, attaches,
  launches a shim's activity freeform, turns
  a change of a task's top orientation into its `ORIENTATION` record (as
  it does `setRequestedOrientation`, so aim-display keeps the one policy:
  the window's proportions, fitted into the screen), takes the desktop to
  be the home task (activity type `HOME`) rather than a task that fills the
  display (a fullscreen task about to become freeform fills it too), and no
  longer commits bounds by moving a task a pixel and back. A placed task's
  window takes its bounds then: a change outside aim-windows' own requests
  (`am task resize`, an app's own `resizeTask` caller) reaches no task
  listener, so before `onTaskPlaced` the window waited for an unrelated
  stack change (#649). And a `LAUNCH`'s splash on the Mac ends (`DRAWN`)
  once the task's window shows the task placed, that is with its starting
  window, not at the app's first frame.

## 5. Starting windows

With an organizer, WM asks it for every starting window
(`TaskOrganizerController.addStartingWindow`) and draws none itself. The
shell's are WMShell's splash screens (`Shell/startingsurface`), made by
`StartingWindows` on the shell's thread:

- **Which.** WMShell's choice for phones
  (`PhoneStartingWindowTypeAlgorithm`) from the request's
  `startingWindowTypeParameter`: a splash screen for a new task, a cold
  start or a switch to an activity not created yet; a solid color one when
  the app asks for it, or for a switch WMShell has no snapshot for; the
  legacy one (the theme's window background) for an app that opts out of
  the SplashScreen API's style. A switch WMShell shows a task snapshot for
  gets none: the task's own surface still shows its last frame, and the
  Mac's window keeps it. Windowless starting surfaces (predictive back,
  not used) get none.
- **What.** The app's package context with the splash screen theme (the
  launch's, else the activity's, else `Theme.DeviceDefault.DayNight`), in
  the task's night mode. Its `windowSplashScreenBackground`, else its
  window background's color, else `colorBackground`; its
  `windowSplashScreenAnimatedIcon` (scaled by 1.2 without an icon
  background), else the activity's icon, at `starting_surface_icon_size`,
  in the icon shape (`config_icon_mask`) over its
  `windowSplashScreenIconBackgroundColor`; its
  `windowSplashScreenBrandingImage` at WMShell's 200 x 80 dp. An Animatable
  icon plays in a surface of its own and goes to the app with the view
  (`SplashIcons`, ported from WMShell's `SplashscreenIconDrawableFactory`).
  An adaptive activity icon is drawn whole, not by WMShell's foreground
  selection and cached rendering (#674). `SplashScreenView.Builder` (the framework's, which
  the app's copy is built with too) makes the view; a
  `TYPE_APPLICATION_STARTING` window on the activity's token
  (`WindowManagerGlobal.addView`, with WMShell's layout flags) shows it.
- **When.** A new task the player will make freeform gets its starting
  window only once it is freeform (after its transition request is
  answered): a splash added to the fullscreen task would be resized by
  the conversion, and WM does not hand a resized splash to its app
  (`StartingData.mResizedFromTransfer`, `wm/WindowState.java:2177-2184`).
- **The SplashScreen API.** An app with
  `SplashScreen.setOnExitAnimationListener` gets the view: WM asks
  `copySplashScreenView` when the app has drawn, the shell answers
  `onSplashScreenViewCopyFinished` with a `SplashScreenViewParcelable`
  (null when the view cannot be copied: a legacy splash screen), and WM
  hands it to the app, whose listener runs; once the app has it attached,
  WM removes the starting window, and the app's removal of the view comes
  back as `onAppSplashScreenViewRemoved`.
- **Removal.** `removeStartingWindow` removes the window at once. WMShell's
  reveal animation (the icon fading, the app sliding in) is not played:
  windows are animated by the Mac, and a reveal leash WM hands over is
  WM's own object, which it cancels when the starting window goes.

The Mac's splash (windows.md, "App shims") then covers only the time
before the task's window exists: aim-windows ends it when the window shows
the placed task (section 4).

## 6. Picture-in-picture (D3 stage 2)

The lightweight shell declares no `android.software.picture_in_picture`
for now (D3), so what follows is in place but not reached: PinnedStackTests
skip. Declaring the feature again is the step that turns it on, gated by
those tests.

The image's `enablePip2` is off (a read-only flag: `isPip2ExperimentEnabled`
returns false in the image's `services.jar`), so entry is WM's legacy path
under shell transitions (`ActivityTaskManagerService.enterPictureInPictureMode`,
`wm/RootWindowContainer.java:2048-2240`): WM moves the activity into a new
root task, sets its windowing mode pinned with the old task's bounds, marks
the activity `mWaitForEnteringPinnedMode` and requests a `TRANSIT_PIP`
transition. The rest is WMShell's (`Shell/pip/PipTransition.java`): its
answer to the request (`augmentRequest`, the alpha entry) or its finish
WCT (the bounds entry) gives the task its PiP bounds and the activity the
pinned mode (`setActivityWindowingMode(UNDEFINED)`), the step that clears
the wait and calls the app's `onPictureInPictureModeChanged`
(`wm/ActivityRecord.java:8310-8316`). With the shell's player today the
entry stops at the first half: the pinned task keeps the app's bounds and
the app never hears it is in PiP.

**The shell's part.** The player answers a `TRANSIT_PIP` request whose
trigger task is pinned as `augmentRequest` does: a WCT that sets the
task's PiP bounds and the activity's windowing mode, applied in the same
transition, with nothing animated. A task the system moved into PiP
within another transition (an auto-enter while the app leaves) gets the
same WCT in a transition of the shell's once that one is ready. The
bounds are WMShell's
`PipBoundsAlgorithm` defaults: the params' aspect ratio clamped to
`config_pictureInPictureMinAspectRatio`..`MaxAspectRatio` (else 16:9), a
size of 23 % of the display's shorter side but at least
`default_minimal_size_pip_resizable_task`, at the bottom right 16 dp
inside the display (`PipBounds`, ported from `PipBoundsAlgorithm` and
`PhoneSizeSpecSource` with the image's SystemUI values; without SystemUI
the display has no system bars to keep clear of). A later
`setPictureInPictureParams` with another aspect ratio (`onTaskInfoChanged`)
resizes the task the same way, keeping its bottom right corner. Leaving
PiP is the platform's: an app's own expand (`moveTaskToFront`, a start into
the task) leaves it through WM (`Task.java:4845-4865`), and the shell
answers the transition like any other.

**The Mac's part.** A pinned task is a window of its own, as any task with
bounds: aim-windows reports it, and with the task's windowing mode (the
fourth argument of `onTaskChanged`) a `PINNED` record (`UNPINNED` when it
leaves), on which aim-display makes the window float
(`NSFloatingWindowLevel`, on all spaces, its content aspect ratio fixed).
Its close button removes the task, as WMShell's dismiss does. Moving or
resizing it goes to the shell (`IWindowShell.setPipBounds`), since
`resizeTask` refuses a pinned task; the window takes the bounds once the
task is placed. Its place is not remembered as the app's (#635). The
app's own expand works as above; the Mac has no expand control of its
own yet (#673).

**The gate.** CtsWindowManagerDeviceOther's `PinnedStackTests`, and
`ActivityLifecyclePipTests` of CtsWindowManagerDeviceActivity, on the
lightweight shell against the default image; the tests of WMShell's PiP
menu and its gestures (`PipMenuActivity`, double tap) do not apply and are
listed as expected differences.

## 7. Coexistence with the default image

The default image keeps SystemUI and WMShell, which register their own
organizer and player when SystemUI starts. Both registrations are
last-wins (`wm/TaskOrganizerController.java:569`,
`wm/TransitionController.java:380-382`), so two shells would take tasks
from each other. The shell is therefore off unless the image says
otherwise: `ro.vendor.aim.lightweight_shell` is set by
`/vendor/etc/init/aim-lightweight-shell.rc`, which only the lightweight
shell variant adds (`image/variants/lightweight-shell.toml`). Without it,
`DeviceServices` creates nothing, `aim.window_shell` does not exist, and
aim-windows sets the display freeform and commits bounds as before.

## 8. Failure modes

| Failure | Effect | Handling |
| --- | --- | --- |
| A request or ready transition left unanswered | the transition stays collecting or playing; later ones queue behind it, and the display stops changing | every request and ready transition is answered in a `finally`, on the shell's thread |
| The shell's thread blocked | as above | it only applies transactions and makes one-way calls; nothing waits on WM from WM's thread |
| aim-windows dies | new tasks stay fullscreen (one fills the display) | aim-windows restarts with zygote; a crash of it alone is a bug to fix, not a mode |
| system_server restarts | everything registers again with the new one; aim-windows restarts and attaches again (its rc) | |
| SystemUI registers too (a mixed image) | the last one wins | the property is only in the variant |
| A bounds change outside any transition | the organized task's surface stays where it was (WM no longer moves it) | none known with a player; a check compares window and surface positions (section 9) |
| A move per drag event | each is a transition waiting for a redraw | measured; the leash drag of section 3 if it lags |
| Persisted launch params from a freeform-display boot | #613 for those components | fresh data directories; #636 before the switch |
| The app's resources fail while its starting window is made | no starting window, as for a type of none; the transition waits for the app's first frame | logged; the Mac's splash covers the launch until then |

## 9. Verification

Per change, in the variant (`cargo aim build --variant lightweight-shell`,
window mode, a fresh data directory):

- `dumpsys activity containers`: the default display area fullscreen,
  app tasks freeform; `dumpsys window` names the registered organizer and
  player (`TaskOrganizerController`, `TransitionController`).
- #613: `am start -n` of a running task's activity, of a singleTask one,
  a notification's content intent: `am stack list` bounds unchanged, the
  Mac window in place.
- #593: an activity with a fixed manifest orientation started in a
  running task of the other orientation turns the window; finishing it
  turns it back.
- D6: a HOME intent from an app hides it; a new fullscreen task does not.
- The integration gate and the app checks (Settings, Chrome, Calculator,
  the tracked games), window moves and resizes, and the surface position
  of each task against its bounds (`dumpsys SurfaceFlinger`).

**Measured** (2026-10-01, the variant, window mode, first boots of fresh
data directories; ops checks organizer-1 and organizer-3). The shell
registers about 1 s before `sys.boot_completed`. A second `am start` of a
running singleTask activity leaves its task's bounds identical (before:
+240,+147). A manifest-landscape activity started in a running portrait
task turns it from 824x1464 to 1464x824 and back when it finishes. Shim
launches, starts from `am start` and a start that joined another
transition all open freeform. Every task surface's display frame equals
its bounds (`dumpsys SurfaceFlinger`), and no FATAL, WTF or ANR line
appears. An `am task resize` reached the Mac window only after an
unrelated task stack change (#649): no task listener hears a bounds
change, so the shell now reports each placed task (section 4).

CTS, before (the default image), the variant without the shell, and with
it, as for the other steps ([system-services.md](system-services.md)); the
modules and their helper apps are pinned in `upstream/cts.lock`
(`tools/cts-module.py`):

| Module | What it covers here |
| --- | --- |
| CtsWindowManagerDeviceActivity | starts, launch params, lifecycles (freeform included), visibility, config changes |
| CtsWindowManagerDeviceAm | `am start` options (launch bounds, windowing mode) |
| CtsWindowManagerDeviceWindow | multi-window, window metrics of freeform tasks |
| CtsWindowManagerDeviceAnimations | manifest `<layout>`, splash screens, activity transitions and their selection |
| CtsWindowManagerDeviceTaskFragment | task fragment organizers inside organized tasks, under shell transitions |
| CtsWindowManagerDeviceOther | lock task, PiP (declared unavailable, D3) |
| CtsWindowManagerDeviceInsets, -Keyguard, -BackNavigation | insets of freeform tasks, the keyguard's absence, back without predictive back |
| CtsWindowManagerDeviceMultiDisplay, -Display | tasks on other displays stay the platform's (the shell organizes, and changes nothing, off the default display) |

The shell replaces WMShell only once these match the default image's
results (or the difference is listed and explained) and the app checks
pass (ADR 0013).

**Measured, tier 1** (2026-10-01, window mode, fresh data directories;
ops checks wmshell2-1 to wmshell2-5): AmStartOptionsTests,
SplashscreenTests, PictureInPictureParamsTest, ActivityLifecycleFreeformTests,
ActivityStarterTests, StartActivityTests, FreeformWindowingModeTests,
TaskFragmentOrganizerTest and TransitionSelectionTests pass on the
lightweight shell as on the default image. MultiWindowTests fails
testVisibilityWithTranslucentAndTopFinishingActivity on both, and the
default image also fails two others and
FreeformWindowingModeTests.testFreeformWindowManagementSupport.
PinnedStackTests and ActivityLifecyclePipTests skip on the lightweight
shell, which declares no PiP (D3). Before the explicit windowing modes and
the held starting windows, SplashscreenTests failed 14 of 17,
TaskFragmentOrganizerTest 2 and MultiWindowTests 3.
