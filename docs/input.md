# Input (ADR 0012, phase P5)

Input is not a HAL. The display server's window (`aim-display`,
[composer.md](composer.md)) turns its AppKit events into Linux **evdev**
devices, the syscall layer shows them as `/dev/input/eventN`, and the
original inputflinger (EventHub in system_server) reads them as it reads a
kernel's. Nothing in the guest is ours except three `.idc` files.

```text
NSView events ──▶ translate::Input ──▶ server::Devices ──unix socket per open──▶ sys::evdev ──▶ EventHub
 (aim-display,   (points → pixels,     (one listening socket                   (linux-run:     (original
  main thread)    keys, pointer)        per device, input-core rules)           /dev/input)     libinputreader)
```

| Piece | Where |
| --- | --- |
| Devices, protocol, translation (library) | `crates/aim-host-display/src/input/` |
| AppKit capture (content view, window delegate) | `crates/aim-host-display/src/bin/aim-display/input.rs` |
| A window host's input, applied in the server | `bin/aim-display/hosts.rs` (`apply`) |
| `/dev/input/eventN`, ioctls, `/sys` nodes | `crates/aim-linux-abi/src/sys/evdev.rs` |
| Device configuration | `image/vendor/usr/idc/aim-*.idc` → `/vendor/usr/idc/` (`image/overlay.toml`) |
| The pointer's sprite as the hardware cursor, the Mac's cursor | `hal/graphics/composer` ([composer.md](composer.md)); `bin/aim-display/cursor.rs`, its hot spot `crates/aim-host-display/src/input/cursor.rs` |

## The devices

| Node | Name | What | Classes EventHub gives it |
| --- | --- | --- | --- |
| `event0` | `aim-touchscreen` | the primary button and the trackpad's pinch and rotation: multitouch protocol B (10 slots), `INPUT_PROP_DIRECT`, axes in display pixels, resolution from the display's dpi | `TOUCH \| TOUCH_MT` |
| `event1` | `aim-keyboard` | physical keys (`KEY_*`) through a keymap of HID usages, and `KEY_BACK`; `Generic.kl`/`Generic.kcm` | `KEYBOARD \| ALPHAKEY`, built-in keyboard |
| `event2` | `aim-mouse` | the pointer as an absolute mouse: `ABS_X`/`ABS_Y` in display pixels, `BTN_TOOL_MOUSE` (hover), `BTN_RIGHT`, `BTN_MIDDLE`, `BTN_SIDE`, `BTN_EXTRA`, `REL_WHEEL`/`REL_HWHEEL` and their `_HI_RES`; `INPUT_PROP_POINTER` | `TOUCH`, a pointer device: source `MOUSE` |

All are `BUS_VIRTUAL`, vendor and product 0 (no real device identity), so
EventHub finds their configuration by name: `/vendor/usr/idc/<name>.idc`.
All three are `device.internal`; the keyboard is `keyboard.builtIn`
(Android then treats the Mac's keyboard as the device's hardware
keyboard); the mouse is `touch.deviceType = pointer`. Key layout and
character map are the image's `Generic.kl` and `Generic.kcm`.

### Decision: the primary button is a finger, the rest a mouse

- **Click and drag** (user's decision, #214). Phone apps are touch-first:
  the primary button is a touch at the absolute point under the cursor, as
  the Android emulator does.
- **Everything else the pointer does** is an absolute mouse at the same
  pixel: a Wacom-style puck (`BTN_TOOL_MOUSE` with absolute axes), which
  Android's TouchInputMapper runs in pointer mode and reports as
  `SOURCE_MOUSE` with the cursor position it is given
  (PointerChoreographer's "absolute mouse"). A relative Linux mouse
  (`REL_X`/`REL_Y`) would draw a second pointer that drifts from the Mac's
  (Android's acceleration on top of the Mac's, clamping at the window
  edge); this one is where the Mac's cursor is, always.
  - **Hover.** The tool is in range while the cursor is over a window
    (`mouseMoved:`, `mouseEntered:` through a tracking area, and the
    right and other buttons' drags) and leaves with `mouseExited:`:
    `ACTION_HOVER_ENTER`/`MOVE`/`EXIT`, so views highlight and show
    tooltips.
  - **Buttons.** Right is `BUTTON_SECONDARY` (context menus: a text field
    shows its menu), middle `BUTTON_TERTIARY`, and the back and forward
    buttons (`buttonNumber` 3 and 4) `BUTTON_BACK` and `BUTTON_FORWARD`,
    which Android turns into `KEYCODE_BACK` and `KEYCODE_FORWARD`.
  - **Scrolling** goes to the window under the pointer (`ACTION_SCROLL` at
    the mouse's position), on both axes, with `REL_WHEEL_HI_RES` and
    `REL_HWHEEL_HI_RES` (120 per unit) and whole units. A unit is what
    Android scrolls for `AXIS_VSCROLL` 1.0, 64 dp (128 pixels at the
    image's 320 dpi), so a trackpad's precise deltas (points, turned into
    display pixels) move the content as far as the fingers; its momentum
    events keep going as macOS sends them. A wheel's lines are units. The
    sign follows the Mac's setting (natural scrolling): content moves where
    it moves on the Mac. Android's own wheel acceleration still applies.
  - **The pointer icon** Android draws for a mouse is a cursor layer
    (SpriteController's `eCursorWindow`); the composer HAL keeps it
    `CURSOR` composition, the display's hardware cursor, which is the
    Mac's own cursor, so it is not drawn into the frame. Screenshots
    SurfaceFlinger renders itself (`screencap`) still show it.
    The Mac's cursor shows the layer's image, as a virtual GPU's host
    cursor does: the composer passes on its buffer and position
    (`FN_CURSOR`), and the display server reads the pixels once the
    acquire fence signals and makes them an `NSCursor` for its views and
    the window hosts' (`cursorUpdate:`). Whatever icon Android picks is
    the cursor: the arrow, the I-beam over text, the hand over links,
    resize arrows, the wait icon, and an app's own `PointerIcon` bitmaps.
    The image is display pixels, sized in points by the view's pixels per
    point, so a Retina window shows its pixels one to one. Without a
    cursor layer the views show the arrow.
    - **Hot spot.** A composer is not told an icon's hot spot (a DRM
      cursor plane is; HWC has no call for it). The layer lies at the
      pointer's position minus the hot spot, and that position is one the
      mouse device reported, so the server finds it
      (`input::cursor::Hotspots`): the offset that puts the most of the
      image's recent positions on positions the mouse had, on a tie the
      image's known one, else one as many events old as the last image
      showed. It is exact at once when the mouse rests, and while it
      moves once an image was seen at rest; each image's hot spot is
      kept by its pixels' hash.
- **Pinch and rotate** (`magnifyWithEvent:`, `rotateWithEvent:`) are two
  fingers on the touchscreen (slots 0 and 1) centered on the pointer,
  40 mm apart at first (above Android's 27 mm minimum scaling span), which
  spread with the magnification and turn with the rotation, as on a
  phone; they lift when both gestures end. ChromeOS does the same.
- **Smart zoom** (a two-finger double tap, `smartMagnifyWithEvent:`) is a
  double tap of the primary button where the pointer is, which zooms as
  on a phone: two 40 ms taps 70 ms apart, ending at the gesture's time
  (GestureDetector's double tap is a second down 40 to 300 ms after the
  first up). The taps are stamped before the gesture, which is when they
  happened; EventHub takes a time in the future as the present.
- **Keys.** macOS virtual key codes map to the key at the same place on a
  PC keyboard (ANSI, ISO and JIS keys; Option is Alt, JIS Eisu/Kana are
  `KEY_HANJA`/`KEY_HANGEUL`, which `Generic.kl` calls EISU/KANA), through
  the keyboard's keymap: each key's scan code is its HID usage (keyboard
  page 7; Fn Apple's 0xff0003), as hid-input reports one, and
  `EVIOCSKEYCODE` remaps it. Characters are Android's: its key character
  map for the keyboard, and its input method. Modifiers come from
  `flagsChanged:` and tell left from right by the device-dependent flag
  bits; Caps Lock goes down and up once per toggle. The keyboard has no
  `EV_REP`: Android repeats keys itself, so AppKit's repeats are dropped
  (EventHub's `EVIOCSREP` gets `ENOSYS`, as with any keyboard without
  kernel repeat). Function keys are `KEY_F1` to `KEY_F20` when macOS sends
  them as such (with Fn, or with "Use F1, F2, etc. keys as standard
  function keys"); otherwise macOS handles them itself.
- **Command is the shortcut key**, as on the Mac: Cmd+C, V, X, A, Z (and
  every other Cmd+key) press Ctrl, the key and release both, which is
  Android's copy, paste, cut, select all and undo; each AppKit repeat does
  it again. Cmd+Left and Right are Home and End, Cmd+Up and Down
  Ctrl+Home and Ctrl+End; with Shift held they select, as on the Mac.
  Cmd+Delete deletes to the line's start (Shift+Home, then Backspace:
  Android has no key for it) and Cmd+Forward Delete to its end.
  Command itself never reaches Android, whose Meta
  tap (all apps) and Meta+letter shortcuts it would trigger. In window
  mode Cmd+W closes the window (its task) and Cmd+Q quits the app
  ([windows.md](windows.md)).
- **Option is Alt**, except for the Mac's word keys: Option+Left and
  Right move by word and Option+Delete and Forward Delete delete one,
  which are Android's Ctrl with the key (`ArrowKeyMovementMethod`,
  `BaseKeyListener`; its Alt there moves to the line's edge and deletes
  the whole line). The held Alt is released around them. With Shift they
  select by word. Each AppKit repeat does it again.
- **Back.** Cmd+[ and a two-finger swipe to the right (when "swipe between
  pages" is on) press the keyboard's `KEY_BACK`; the mouse's back button
  is the mouse's. A trackpad gesture whose first 8 points go mostly right
  is the swipe: it does not scroll, nor does its momentum; any other
  gesture scrolls. Esc stays Esc (user's decision); Android 16 does not
  turn an unhandled Esc into Back ([windows.md](windows.md), "Back").
- When the window stops being key, the fingers lift and every key and
  button is released, so nothing stays down in the guest.

### Layouts

Keys are physical; the characters are Android's: the keyboard's key
character map (`Generic.kcm`) with the overlay of the layout Android's
`KeyboardLayoutManager` picks for it, which also remaps key codes (under
Dvorak the key at Q is `KEYCODE_APOSTROPHE`, and Ctrl+the key at I is
Ctrl+C, as Cmd+C is on the Mac).

- **The Mac's layout picks it.** guest-init reads the Mac's current
  keyboard layout (`AppleCurrentKeyboardLayoutInputSourceID` of
  `com.apple.HIToolbox`) and sets `vendor.aim.mac.keyboard_layout` to the
  InputDevices layout with the same characters on the unmodified and Shift
  levels (`mac::KEYBOARD_LAYOUTS`: Dvorak, Colemak, US International - PC,
  British - PC, French - PC, German, Russian, Russian - PC), else US
  English ([mac-settings.md](mac-settings.md)). At `sys.boot_completed`
  and on each change, init runs `aim-keyboard layout NAME`
  (`daemons/keyboard`, `/system_ext/bin`), which sets InputManager's layout
  override for the built-in keyboard
  (`IInputManager.setKeyboardLayoutOverrideForInputDevice`, by the
  descriptor EventHub gives a built-in device of its name), as a device's
  vendor component would. A layout chosen in Android's Settings for an
  input method wins over it.
- **Input methods' layouts.** Korean 2-Set (`2SetHangul`) is US English at
  the key level. Its Hangul come from Android's input method composing
  from the keys (the image's Gboard does, with Korean as its language:
  G K S R M F give 한글), not from the Mac's input method (#23).
- **ISO keyboards.** An Apple ISO keyboard's key left of 1
  (`kVK_ISO_Section`) and key right of left Shift (`kVK_ANSI_Grave` there)
  are in each other's place relative to a PC's. When the Mac's keyboard is
  ISO (`KBGetLayoutType`, at start), the display server remaps the two in
  the keyboard's keymap (`KEY_GRAVE`, `KEY_102ND`), as Linux's hid-apple
  does.
- **JIS keyboards** send their own keys (`KEY_YEN`, `KEY_RO`, Eisu, Kana),
  but InputDevices has no JIS character map, so the punctuation follows US
  English.

### Coordinates

In window mode each window maps its points one to one onto its task's part
of the display ([windows.md](windows.md)). In device mode the display mode
is the window's content in backing pixels, and the layer
shows it aspect-fitted (`resizeAspect`) when the window is resized. A view
point (points, origin bottom left) maps to a display pixel by the fit's
scale and offset, with y flipped (`translate::to_display`). A press on the
letterbox bars is ignored; a drag leaving the picture is clamped to its
edge. The mapping never uses the backing scale directly, so it holds at any
window size and on any screen.

### Time

Events carry the guest's `CLOCK_MONOTONIC`, which is `mach_absolute_time`
(`aim_hostcall::clock`): an `NSEvent` timestamp (`mach_absolute_time` in
seconds) needs no conversion. A packet's events share one time, as the input
core stamps them. The syscall layer converts to the clock each open file
chose (`EVIOCSCLOCKID`: `CLOCK_REALTIME` by default, `CLOCK_MONOTONIC`,
`CLOCK_BOOTTIME`).

## Delivery: a socket per open file

The devices of the display server at `SOCKET` are listening Unix sockets
`SOCKET.input/eventN`. `linux-run --display SOCKET` shows that directory as
the guest's `/dev/input`, so the input devices need no option of their own:
they belong to the server that owns the window.

- **Open** connects (by name from the directory, so a long directory path
  does not hit `sun_path`) and sends a hello; the server answers with the
  device's descriptor (identity, capabilities, current state) and then
  writes whole packets of 16-byte records (time, type, code, value). The fd
  the guest gets is that socket: poll and epoll see it readable while a
  packet waits and hung up when the device goes away, with no layer state.
- **Read** returns whole `struct input_event`s (24 bytes, 64-bit `timeval`),
  as many as fit; a buffer shorter than one event is `EINVAL`, an empty
  non-blocking one `EAGAIN`, a device gone or revoked `ENODEV`.
- **Write** injects events into the device, as on Linux; they reach every
  open file through the same rules.
- The server applies the **input core's rules**: undeclared events and
  unchanged values are dropped, an empty packet is not sent. A client whose
  socket is full loses the packet and gets `SYN_DROPPED` before the next
  one; a slow reader never stalls the window.
- **Control requests.** What an open file asks of the device goes on a
  separate connection, naming the client (its id from the descriptor):
  grab, the device as it is now (`OP_DESCRIBE`: capabilities, axes and
  state, which `EVIOCSABS` and `EVIOCSKEYCODE` change for every open
  file), axis changes, event masks, the keymap, and flushing the client's
  queue. Each answers a Linux errno.
- **Masks** are the server's, per client: masked events are never queued,
  so poll stays exact, and a packet left empty is not sent (evdev drops an
  empty `SYN_REPORT`).
- **Hotplug.** A socket is bound and listening before it is renamed into
  the directory, so a client that sees a node can open it. The server
  holds a lock (`flock`) on the directory while its devices exist, removes
  the nodes when it quits (window closed, `SIGTERM`, `SIGINT`) and removes
  stale ones when it starts; inotify on `/dev/input` reports both, and the
  open files hang up. A server that dies without cleanup (`SIGKILL`) leaves
  its directory unlocked: the first client that finds a node then (an open
  refused, a listing of `/dev/input`, a device that hung up) removes them,
  and the open fails with `ENODEV`.

## Syscall layer (`sys::evdev`)

| Request | Answer |
| --- | --- |
| `EVIOCGVERSION`, `EVIOCGID`, `EVIOCGNAME` | `0x010001`; the device's id; the name and its NUL, cut to the buffer |
| `EVIOCGPHYS`, `EVIOCGUNIQ` | `ENOENT` (virtual devices have neither) |
| `EVIOCGBIT(ev)`, `EVIOCGPROP` | the bitmap, `BITS_TO_LONGS(max) * 8` bytes at most (96 for keys; the keys as the keymap has them now); `EINVAL` for types evdev does not answer |
| `EVIOCGABS(axis)` | range, resolution and value as the device has them now (Linux does not flush for it); `EINVAL` on a device without axes |
| `EVIOCSABS(axis)` | sets them for every open file, value included; a struct shorter than `input_absinfo` sets no resolution; `EINVAL` for `ABS_MT_SLOT` and on a device without axes |
| `EVIOCGKEY`, `EVIOCGLED`, `EVIOCGSND`, `EVIOCGSW`, `EVIOCGMTSLOTS` | the state as of the events this open file has read (see below) |
| `EVIOCGKEYCODE`, `EVIOCSKEYCODE`, and `_V2` | the keyboard's keymap, as hid-input's: scan codes are HID usages (1, 2 or 4 bytes; `INPUT_KEYMAP_BY_INDEX` by position); setting updates the key bitmap and releases a held key the device no longer has; `EINVAL` for an unknown scan code, a key above `KEY_MAX`, a `len` above 32, and on devices without a keymap |
| `EVIOCGMASK`, `EVIOCSMASK` | this open file's mask of an event type (all set until one is given; the buffer past the kernel's bitmap zeroed); a type without codes to mask reads zeroes and takes any mask; `EFAULT` for a missing buffer |
| `EVIOCGRAB` | `0`, `EBUSY` while another open file (or this one) holds it, `EINVAL` releasing one not held |
| `EVIOCREVOKE` | shuts the connection: reads and requests `ENODEV`, poll hangs up |
| `EVIOCSCLOCKID` | `REALTIME`, `MONOTONIC`, `BOOTTIME`; else `EINVAL`. A new clock drops what is queued and queues `SYN_DROPPED` (only if something was) |
| `EVIOCGREP`, `EVIOCSREP`, `EVIOCSFF`, `EVIOCRMFF` / `EVIOCGEFFECTS` | `ENOSYS` (no repeat, no force feedback) / 0 |
| other `'E'` requests | `EINVAL` |

The state requests answer from the descriptor's state updated by every
event the open file has read. That is the device's state minus what is still
queued, so it is consistent with the queue; Linux gets the same by dropping
queued events of the type it reports. Two cases take the device's state
instead, as Linux's clients see it: after `SYN_DROPPED` (the lost packets
changed it; a client resynchronizes with these requests), and for the
codes the open file's masks drop (they are never queued).

- A clock change with events queued sends a flush request; the server
  writes a `FLUSH` record (not an event type) and `SYN_DROPPED` into the
  client's socket, and the request reads and drops everything up to the
  `FLUSH` record (applying it to the state). `SYN_DROPPED` stays queued,
  stamped in the new clock.
- **Across exec.** The open file is the socket, which exec keeps; the
  socket carries a marker (the `SO_LINGER` time, as sync files and
  SEQPACKET sockets do) and its client id, clock and access mode (its
  receive timeout, unused: reads never block in `recv`). After exec (or
  an fd received by `SCM_RIGHTS`) `fdtab::adopt` finds the device from the
  socket's peer name, `eventN`, and asks the server for its state: the
  new image's reads, writes, grab and requests are the same open file's.
- `stat` shows a node as the character device `13:(64+N)`, `root:input`
  0660 (as `ueventd.rc` makes them), and `readdir` gives `DT_CHR`.
  `/proc/self/fd/N` names the node.
- `/sys/dev/char/13:M`, `/sys/class/input/{eventN,inputN}` and
  `/sys/devices/virtual/input/inputN/eventN/dev` exist. EventHub resolves
  the first with `realpath` and takes `/sys/devices/virtual` as the device's
  sysfs root, as for a Linux virtual device (no batteries, lights or HID
  country code). The `realpath` works because a synthesized `/proc` or
  `/sys` directory fd stats and reads back (`/proc/self/fd`) as its path.

## Evidence

| Test | Shows |
| --- | --- |
| `aim-host-display` unit tests | key map and usages, modifiers, view-to-pixel mapping (Retina, resize, letterbox), input-core rules, masks, packets, state for late openers, grab, injection, hotplug removal, stale nodes; the mouse's hover, buttons and two-axis scrolling in units; Cmd shortcuts as Ctrl; the Mac's text shortcuts (Option's word keys as Ctrl with Alt lifted, Cmd+Delete, Cmd+Shift selection); smart zoom as a double tap within GestureDetector's window; the cursor's hot spot at rest, settling while the mouse moves, and carried to a new image by the lag; the swipe that is Back and does not scroll (nor its momentum) against one that scrolls; pinch and rotation as two fingers; keymap remapping with the held key released, axis changes, flush |
| `aim-linux-abi` `tests/input.rs` | the original `getevent -lpi` lists the three devices with their capabilities and held state (`BTN_TOUCH*`, `KEY_A*`, the mouse's axes and `INPUT_PROP_POINTER`), and `getevent -l` prints the packets `translate::Input` makes from a touch, a drag clamped at the edge, a key (AppKit repeat dropped), right Shift, a hover, a right click, a wheel line and a trackpad's horizontal pixels; `getevent -t` opens devices hotplugged under its inotify watch, with the event's own monotonic time; a dead server's nodes are removed when getevent lists them |
| `tests/ndk.rs` `evdev` (`t_evdev.c`, Linux UAPI headers) | every ioctl above with Linux's return values and errnos (masks filtering one open file's queue while its key state still has the masked key; axes changed for another open file; keymap by scan code and index, remapped and restored; a clock change dropping the queue for `SYN_DROPPED`), read/write/poll/epoll semantics, clocks, grab, revoke, the open file across `execve` (non-blocking, clock, grab, `/proc/self/fd`), stat/fstat/readdir/`/proc/self/fd`, `realpath` of `/sys/dev/char` |
| `tests/ndk.rs` `eventhub` (`t_eventhub.cpp`) | the image's `libinputreader.so` EventHub scans `/dev/input`, classifies `TOUCH \| TOUCH_MT`, `KEYBOARD \| ALPHAKEY` (built-in keyboard, from its `.idc`; `Generic.kl`, `Generic.kcm`) and the mouse `TOUCH`, finds sysfs root `/sys/devices/virtual`, and reads a key written into the keyboard |
| A window-mode boot ([boot-status.md](boot-status.md), "Pointer, scrolling and shortcuts") | the mouse in pointer mode with source `MOUSE`; hover highlighting a Settings row, two-finger scrolling of Settings and both axes in `getevent -lt`, a right click opening a text field's context menu, Cmd+A/C/V duplicating its text, a pinch with rotation as two pointers, a rightward swipe as Back with no scrolling; the pointer sprite `CURSOR` in the composer and absent from the presented frame |

`aim-display`'s AppKit handlers were not driven by real clicks and keys
(that would post events to the user's session). In the boot the input went
through the server's own path from a window host's records
(`hosts::apply`, the same `translate::Input` calls the handlers make).

## IME (#23)

Keys go out as physical keys, and Android's own input method composes
text from them. That is enough for Latin layouts and for the Korean,
Japanese and Chinese input methods that compose from key codes: the
image's Gboard, with Korean as its language, turns G K S R M F into 한글
(its language switches with Ctrl+Space). The Mac's input method (its
candidate window, dictionaries, dictation, emoji picker and input-source
switching) is not used. The options, none built yet (an input method is
a service in an app: adding one is a new system component, the user's
decision):

| Option | How | For | Against |
| --- | --- | --- | --- |
| **A. The Mac's input method through a bridging IME** (recommended) | The content view adopts `NSTextInputClient` and sends `keyDown:` through `interpretKeyEvents:`; keys the input method does not consume still go out as evdev keys. Marked and committed text go to a minimal Android IME (an `InputMethodService` in an APK in the derived image, the default IME) over the display server's connection, which applies them with `InputConnection.setComposingText` and `commitText`, and reports `onUpdateCursorAnchorInfo` so `firstRectForCharacterRange:` puts the candidate window at the caret (pixels mapped back to points). Non-editor focus keeps the raw key path, so games still get keys. | Every Mac input source, its candidates and dictionaries, dictation and the emoji picker; switching as on the Mac; ChromeOS (ARC) and Windows Subsystem for Android bridge the same way | A new APK (Java, the one framework class an IME must extend) replacing Gboard as the IME; the IME's protocol to the host; `EditorInfo` and selection kept in sync both ways |
| B. Android's IME, following the Mac's input source | Keys as now; a system tool (as `aim-keyboard`) switches Gboard's subtype when the Mac's input source changes (`com.apple.inputmethod.Korean` → Korean) | No new component; Gboard's composition works today | Gboard's candidates and dictionaries, not the Mac's; subtype switching needs the IME's own subtype ids; the Mac keeps intercepting its switching keys (Caps Lock, Ctrl+Space), whose Caps Lock still reaches Android |
| C. Committed text as key events | `injectInputEvent` of `ACTION_MULTIPLE` key events with characters | Nothing in the guest | No composing text, no candidates; apps handle it unevenly; deprecated |

A needs an ADR on input method ownership (a host input method provider
and an APK in the derived image).
