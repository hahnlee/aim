# Per-task composition in window mode (#692)

In window mode ([windows.md](windows.md)) each Mac window used to show a
crop of the one buffer SurfaceFlinger composed for the whole display. Where
two tasks overlap on the display, a window showed the other task's pixels
whenever the Mac's stacking or placement differed from Android's (Chrome
inside the Clock's window, ops screenshot `switch8-B-chrome`). The
user's decision (#692): the composer HAL answers device composition for the
layers, as a device with overlay planes does, SurfaceFlinger hands it each
layer's buffer and geometry, and `aim-display` composes each Mac window
from its own task's layers. SurfaceFlinger and the framework stay
unchanged.

Evidence is from the pinned sources (`android-16.0.0_r1`;
`sf/` is `frameworks/native/services/surfaceflinger/`, `CE/` its
`CompositionEngine/src/`, `wm/` is
`frameworks/base/services/core/java/com/android/server/wm/`,
`composer3/` is `hardware/interfaces/graphics/composer/aidl/android/hardware/graphics/composer3/`).

## Summary

- **Device mode is unchanged**: client composition, one buffer, one
  window ([composer.md](composer.md)).
- **In window mode the HAL keeps every layer it can draw as DEVICE**
  (buffers of the formats the server imports, solid colors) and sends the
  display server a frame: the layers in z order with their buffer, source
  crop, display frame, transform, plane alpha, blend mode, color and
  visible region. Layers SurfaceFlinger composes itself (`CLIENT`) stay in
  the frame as places where the client target shows.
- **The server attributes each layer** to a task by geometry: WM crops
  every organized task to its bounds, so each of a task's layers lies
  within them; z order settles layers that lie within two tasks. A layer
  within no task is the desktop's (the home and the wallpaper, not shown)
  or a system window's (the IME, toasts, system dialogs), which a panel of
  its own shows (#349).
- **Each window draws only its task's layers**, one render pass per
  window; a window host (an app's shim) gets the frame with its owners and
  does the same. Input is unchanged.
- **Client composition keeps the separation** where it matters: a window
  shows the client target only within the visible region of its own
  `CLIENT` layers.

## 1. What SurfaceFlinger sends a composer that takes DEVICE

Per output layer (`CE/OutputLayer.cpp`, `writeStateToHWC`):

| State | When sent | Where |
| --- | --- | --- |
| `displayFrame`, `sourceCrop` (float), `z`, `transform` | when the geometry changed, or the layer was overridden or skipped | `writeOutputDependentGeometryStateToHWC`, `:521-578` |
| `blendMode`, `planeAlpha` | the same | `writeOutputIndependentGeometryStateToHWC`, `:580-612` |
| `visibleRegion`, `blockingRegion`, `dataspace`, `brightness` | every frame | `writeOutputDependentPerFrameStateToHWC`, `:657-700` |
| `colorTransform`, `damage` | every frame | `writeOutputIndependentPerFrameStateToHWC`, `:722-760` |
| `buffer` (slot, handle once per slot, acquire fence) | every frame, `DEVICE` and `CURSOR` layers only | `writeBufferStateToHWC`, `:864-910` |
| `color` | `SOLID_COLOR` layers | `writeSolidColorStateToHWC`, `:790-801` |
| `composition` | when it changed | `writeCompositionTypeToHWC`, `:912-934` |

So the HAL keeps each layer's state across frames and updates what each
command carries. `z` is the layer's index among the output's layers, in
SurfaceFlinger's flattened z order. A layer that SurfaceFlinger finds fully
occluded (`visibleRegion` empty) is not an output layer at all: its HWC
layer is destroyed (`CE/Output.cpp`, `ensureOutputLayerIfVisible`).

**Release fences.** For each layer SurfaceFlinger releases the buffer it
replaced on the fence the composer returns for that layer, merged with the
client target's acquire fence when there was client composition
(`CE/Output.cpp:1655-1679`); without one the old buffer is released at
once. A destroyed layer's buffer is released on the present fence
(`:1683-1688`). The HAL returns each replaced buffer's release fence as
the present's fence: the server reads a layer's buffers only until the
pass of the next frame that replaced it has finished, and the present
fence signals after that.

**When SurfaceFlinger composes a layer itself** (`CLIENT`, whatever the
composer answers):

| Cause | Where | Here |
| --- | --- | --- |
| Rounded corners | `CE/OutputLayer.cpp:979-982` (`isClientCompositionForced`) | WM sets none without WMShell's decorations (`Task.java:6250` clears them); app-compat letterbox corners (`AppCompatRoundedCorners`) |
| Shadows, stretch, edge extension, borders | `sf/FrontEnd/LayerSnapshotBuilder.cpp:957` | shadows and borders are WMShell's; stretch is a SurfaceView inside an overscrolling list |
| Background blur: the blurring layer and every layer below it | `CE/Output.cpp:859-873` | `FLAG_BLUR_BEHIND` and `setBackgroundBlurRadius` windows (`wm/DimmerAnimationHelper.java:294`); the image allows blur (`ro.surface_flinger.supports_background_blur=1`) |
| A secure layer on a non-secure display, a rotation not a multiple of 90° | `CE/OutputLayer.cpp:370-375` | the display is secure; arbitrary rotations only in animations, which the shell does not play |
| A dataspace the display does not support (HDR) | `:456` | |
| LUTs (the composer reports no overlay properties) | `:322-345` | |
| `setColorTransform` of a layer the composer refuses | `:724-733` | the HAL refuses: CLIENT |
| The developer option "Disable HW overlays" | `CE/Output.cpp:866` | |

The HAL itself asks for `CLIENT` (composer3 allows `DEVICE`/`SOLID_COLOR`
→ `CLIENT` at validation) for: a buffer the server cannot import (YUV
video, `composer3/LayerCommand.aidl` "buffer"), a sideband stream, display
decoration and refresh rate indicator layers, a layer color transform that
is not the identity, and every layer while the display has a color
transform that is not the identity (`composer3/DisplayCommand.aidl`
`colorTransformMatrix`: "it must force all layers to client composition").
It then makes the `CLIENT` layers contiguous in z: a `DEVICE` layer between
two `CLIENT` layers becomes `CLIENT` as well, as hardware composers do, so
the client target has one place in the frame.

**Keeping tasks apart under client composition.** RenderEngine starts the
client target transparent (`renderengine/skia/SkiaRenderEngine.cpp:824`)
and draws each `CLIENT` layer only within its visible region
(`CE/Output.cpp:1508`, `clip`). A window draws the client target at its
place in z, and only within the union of the visible regions of its own
`CLIENT` layers. Where an opaque layer of another task lies above, those
regions exclude it, so no other task's pixels come in. What can still come
in is another task's translucent pixels above one's own client-composed
layer, in the area where both overlap; the other task's window covers that
area on the Mac while the stackings agree.

## 2. Layer to task

**What the composer is told of a layer.** Nothing that names it.
`LayerCommand` (`composer3/LayerCommand.aidl`) has no name and no metadata.
SurfaceFlinger passes `METADATA_TASK_ID` to a composer as the generic
metadata `org.chromium.arc.V1_0.TaskId` (ARC's key,
`sf/SurfaceFlinger.cpp:8262`), but only to a HIDL composer 2.4:
`AidlComposer::setLayerGenericMetadata` returns `UNSUPPORTED`
(`sf/DisplayHardware/AidlComposerHal.cpp:1486-1496`). It would not help
either: a snapshot carries only its own layer's metadata, not its parents'
(`sf/FrontEnd/LayerSnapshotBuilder.cpp:286-304`), and WM sets the task id
only on the task's own surface, an effect layer without content
(`wm/Task.java:3040`), which never reaches the composer.

**What the window shell can report.** It holds each task's leash and
knows its bounds and the tasks' order, but no composer layer id: the
composer's ids are the HAL's own (`createLayer`), and no interface maps a
`SurfaceControl` to one. Reporting a hierarchy per frame would still leave
the matching to geometry.

**Mirrors per task** (a SurfaceFlinger display per task with a mirror of
its leash, as `screenrecord` makes one) would name each layer by its
display, but they need hidden display APIs in system_server, compose every
task twice, and need an output buffer path for each display. Not taken.

**Chosen: geometry against the tasks' bounds, settled by z order.** The
window shell organizes every task and its transitions' finish transactions
crop each task's surface to its bounds (`wm/Transition.java:1126-1160`,
task-organizer.md section 1), so every layer of the task, popups, dialogs,
dims and SurfaceViews included, lies within the task's bounds on the
display. The server knows every task's bounds (the bridge's `TASK`
records). For each layer, in z order:

1. **Desktop.** The layers at the bottom whose frame covers the whole
   display: the home activity's window and a wallpaper. Not shown: the
   Mac's desktop is the desktop.
2. **A task's.** The tasks whose bounds contain the layer's frame. A frame
   equal to exactly one task's bounds (a task's main or starting window)
   is that task's; a frame within one task only is that task's.
3. **Within several tasks** (an overlap, or one task inside another): the
   owner of the nearest layer above it that is within the same tasks, else
   the nearest below, else the frontmost task. A layer between task A's
   and task B's (B above A) that lies within both is either A's (a popup
   above A's window) or B's (a SurfaceView below B's window). Taking B's is
   right in both cases on screen: SurfaceFlinger sends A's layer only where
   it shows through B (it culls layers B's opaque ones cover), and B's
   window, drawn over A's on the Mac as on Android, shows it where Android
   does.
4. **Within no task.** A layer a task owned in an earlier frame (moved
   with its task before the new bounds arrived: the composer's layer ids
   last as long as the layer) keeps that task while the task exists, and
   is hidden once the task is gone (a closing window). Any other layer is
   a system window's, after it has lasted [`GRACE`](#5-fallbacks) without
   a task taking it (a new task's first frames come before its bounds).

## 3. Windows outside tasks

| Window | Where it is on Android | Where it shows |
| --- | --- | --- |
| Popups, menus, spinners, dialogs and dims of an app | children of its activity's window, in its task, cropped to the task | the task's window |
| Starting windows (splash screens) | in the task | the task's window |
| The IME | the display's bottom, layered above its target (`wm/DisplayContent.java`, `assignRelativeLayerForIme`) | a system panel |
| Toasts, system alerts, ANR and other system dialogs, accessibility overlays | above the tasks, at their own place on the display | a system panel |
| A fullscreen task (an explicit fullscreen launch, as WM CTS makes) | over the display | as a desktop layer when it lies at the bottom over the home; a system panel above freeform tasks |
| Home, wallpaper | the bottom of the display | not shown |

A system window that lies entirely within one task's bounds cannot be told
apart from that task's own layers by geometry, and shows in that task's
window: in window mode a toast is placed at the bottom of its app's task,
so an app's toast shows inside its own window (checked with the shell
probe's toast, ops check layers-1).

**System panels.** The system layers are grouped by overlap; each group
gets a borderless, non-activating panel exactly over its bounding box
(display pixels to screen points as for task windows), at the status
window level, on every space, drawing only that group's layers over a
transparent background. A press in a panel is a touch at the display
pixel under it, with no task to bring to the front; the key window stays
the app's. A panel goes when its layers go.

## 4. aim-display

- **Protocol.** `FN_CONNECT` answers the server's mode, so the HAL knows
  window mode. A present in window mode is `FN_LAYERS`: the client target
  (if any), its acquire fence and the layers (`display::Layer`, with
  their buffers' acquire fences) and their visible regions' rectangles.
  The module merges the fences into one acquire fence and sends the
  server one `OP_LAYERS` record followed by the layers and rectangles, with
  the present fence's writer, as `FN_PRESENT` does.
- **Buffers.** Each layer buffer is imported once by its gralloc buffer id
  (`FN_IMPORT`, the memfd path of the client target), reference-counted
  across layers' slots and the client target's, and released when no slot
  holds it. Formats: those the server maps as textures (RGBA/RGBX 8888,
  BGRA 8888, RGB 565, FP16, 1010102); other formats are `CLIENT`.
- **Fences.** The server waits for the merged acquire fence (3 s at most,
  as before), composes, and signals the present fence when every window
  and host has shown the frame (composer.md). Every buffer a frame replaced
  is released on that fence.
- **Composition.** One render pass per visible window (and per panel):
  each of its layers is a quad from its display frame, mapped into the
  window's part of the display, sampling the source crop through the
  layer's transform (flips, then a 90° turn), with its plane alpha and
  blend mode (premultiplied, coverage, none), or its solid color; the
  client target is drawn at its place, cut to the window's `CLIENT`
  regions. What no layer covers is black in a task window and
  transparent in a panel.
- **Window hosts** get each frame's layers with their owners and the
  buffers they lack, and compose their windows the same way.
- **Input** is unchanged: a press is a touch at the display pixel under
  it, and Android's hit testing finds the window there.
- **Refresh.** A window that becomes visible, or a task whose bounds
  arrive, is drawn again from the last frame, attributed again; the
  buffers of the last frame are the ones the composer still holds.

## 5. Fallbacks

- **A layer no task contains** is never drawn into another task's window:
  it keeps the task it had, is hidden when its task is gone, and otherwise
  shows in a system panel at its place on the display after `GRACE`
  (200 ms) during which a task may take it.
- **Client composition** (blur, rounded corners, video in a format the
  server does not map, a display color transform): the client target is
  cut per window as in section 1.
- **Device mode** keeps client composition, which is also the composer's
  answer when the server is not in window mode.
- SurfaceFlinger's layer caching (`debug.sf.enable_layer_caching`), which
  would merge layers of several tasks into one buffer, is off in the
  image and must stay off.

**A task whose surface is not at its bounds (#691).** In the
screenshot `switch8-B-chrome` Chrome's task 12 has bounds [2228,663][3052,2127],
its Mac window lies exactly there, and its content is drawn 241 pixels
further left, its full width (not cut at the bounds): the task's leash
stayed at an earlier position (the cascade's offset is 240 pixels) while
the task's bounds moved. That is the window shell's failure mode "a bounds
change outside any transition" (task-organizer.md section 8): an organized
task's leash is the organizer's to place. Composition cannot correct it:
Android's input windows follow the surfaces, so a touch reaches Chrome
where Android draws it; drawing Chrome's layers elsewhere would put every
touch off by the same offset. Per-task composition shows each layer at its
display frame, where Android shows it and hit-tests it, so the input
mapping (window point → display pixel under it) stays right. It stops the
bleed (Chrome's pixels in the Clock's window); the offset layers lie within
no task and show in a system panel at Android's place until the shell
places the leash at the task's bounds, which is the fix for #691.

## 6. Verification

- **Unit tests** (`aim_host_display::layers`): attribution (overlaps,
  nesting, exact bounds, moves, closing tasks, desktop, system layers), the
  transform's texture coordinates, the disjoint cover of a region.
- **Visual check** (ops, window mode, fresh data directory): two
  overlapping apps (Clock and Chrome, Settings and Calculator), each Mac
  window brought to the front in turn, then the back window moved so it
  overlaps the other differently: screenshots of each window
  (`screencapture -l`) show only its own app. A toast and the IME show in
  panels; a dialog and a popup menu in their app's window.
- **The CTS gate**: window mode, the window shell's modules
  (task-organizer.md section 9: CtsWindowManagerDeviceActivity, -Am,
  -Window, -Animations, -TaskFragment, -Other, -Insets, -Keyguard,
  -BackNavigation, -MultiDisplay, -Display) and the composer's own:
  `CtsSurfaceControlTests` and `android.view.cts.ASurfaceControlTest`
  (present and release fences, buffer latching), with the same results as
  window mode with client composition. Screenshots in CTS come from
  SurfaceFlinger's RenderEngine, not the composer, so the pixels CTS
  checks do not change.
