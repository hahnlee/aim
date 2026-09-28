# Input (ADR 0012, phase P5)

Input is not a HAL. The display server's window (`aim-display`,
[composer.md](composer.md)) turns its AppKit events into Linux **evdev**
devices, the syscall layer shows them as `/dev/input/eventN`, and the
original inputflinger (EventHub in system_server) reads them as it reads a
kernel's. Nothing in the guest is ours except three `.idc` files.

```text
NSView events ──▶ translate::Input ──▶ server::Devices ──unix socket per open──▶ sys::evdev ──▶ EventHub
 (aim-display,   (points → pixels,     (one listening socket                   (linux-run:     (original
  main thread)       keys, wheel)          per device, input-core rules)           /dev/input)     libinputreader)
```

| Piece | Where |
| --- | --- |
| Devices, protocol, translation (library) | `crates/aim-host-display/src/input/` |
| AppKit capture (content view, window delegate) | `crates/aim-host-display/src/bin/aim-display/input.rs` |
| `/dev/input/eventN`, ioctls, `/sys` nodes | `crates/aim-linux-abi/src/sys/evdev.rs` |
| Device configuration | `image/vendor/usr/idc/aim-*.idc` → `/vendor/usr/idc/` (`image/overlay.toml`) |

## The devices

| Node | Name | What | Classes EventHub gives it |
| --- | --- | --- | --- |
| `event0` | `aim-touchscreen` | primary button: one finger, multitouch protocol B (10 slots), `INPUT_PROP_DIRECT`, axes in display pixels, resolution from the display's dpi | `TOUCH \| TOUCH_MT` |
| `event1` | `aim-keyboard` | physical keys (`KEY_*`) and `KEY_BACK`, `Generic.kl`/`Generic.kcm` | `KEYBOARD \| ALPHAKEY`, built-in keyboard |
| `event2` | `aim-wheel` | wheel and trackpad scrolling: `REL_WHEEL`, `REL_WHEEL_HI_RES` | `ROTARY_ENCODER` |

All are `BUS_VIRTUAL`, vendor and product 0 (no real device identity), so
EventHub finds their configuration by name: `/vendor/usr/idc/<name>.idc`.
The touchscreen and keyboard are `device.internal`; the keyboard is
`keyboard.builtIn` (Android then treats the Mac's keyboard as the device's
hardware keyboard); the wheel is `device.type = rotaryEncoder`. Key layout
and character map are the image's `Generic.kl` and `Generic.kcm`.

### Decision: the mouse is a finger, the wheel a rotary encoder

- **Pointer.** Phone apps are touch-first, and the Android pointer must stay
  under the Mac's cursor. A relative Linux mouse (`REL_X`/`REL_Y`, the only
  kind EventHub classifies as a cursor) would draw a second pointer that
  drifts from the Mac's (Android's pointer acceleration on top of the
  Mac's, clamping at the window edge). So the primary button is a touch at
  the absolute point under the cursor, as the Android emulator does.
  Right and middle buttons and hover are not reported yet.
- **Scrolling.** A device with only wheel axes is not a cursor, and
  EventHub drops it unless it is a rotary encoder. `REL_WHEEL_HI_RES` is 120
  per line; mouse wheels report lines, trackpads points (10 points a line).
  The sign follows the Mac's setting (natural scrolling): content moves
  where it moves on the Mac. Rotary scrolling goes to the focused window,
  not the one under the cursor, and is vertical only.
- **Keys.** macOS virtual key codes map to the Linux key at the same place
  on a PC keyboard (ANSI, ISO and JIS keys; Command is Meta, Option is Alt,
  JIS Eisu/Kana are `KEY_HANJA`/`KEY_HANGEUL`, which `Generic.kl` calls
  EISU/KANA). Modifiers come from `flagsChanged:` and tell left from right
  by the device-dependent flag bits; Caps Lock goes down and up once per
  toggle. The keyboard has no `EV_REP`: Android repeats keys itself, so
  AppKit's repeats are dropped (EventHub's `EVIOCSREP` gets `ENOSYS`, as with
  any keyboard without kernel repeat).
- **Back.** The Mac's ways back press the keyboard's `KEY_BACK`: Cmd+[,
  the mouse's back button and a two-finger swipe to the right (when
  "swipe between pages" is on). Esc stays Esc; Android 16 does not turn an
  unhandled Esc into Back ([windows.md](windows.md), "Back").
- When the window stops being key, the finger lifts and every key is
  released, so nothing stays down in the guest.

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

Events carry the guest's `CLOCK_MONOTONIC` (the host's): an `NSEvent`
timestamp (`mach_absolute_time` in seconds) is shifted by the offset between
the two clocks. A packet's events share one time, as the input core stamps
them. The syscall layer converts to the clock each open file chose
(`EVIOCSCLOCKID`: `CLOCK_REALTIME` by default, `CLOCK_MONOTONIC`,
`CLOCK_BOOTTIME`, which is monotonic here as in `clock_gettime`).

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
- **Hotplug.** A socket is bound and listening before it is renamed into
  the directory, so a client that sees a node can open it. The server
  removes the nodes when it quits (window closed, `SIGTERM`, `SIGINT`) and
  removes stale ones when it starts; inotify on `/dev/input` reports both,
  and the open files hang up. A server that dies without cleanup leaves
  nodes that fail to open with `ENODEV` until the next start.

## Syscall layer (`sys::evdev`)

| Request | Answer |
| --- | --- |
| `EVIOCGVERSION`, `EVIOCGID`, `EVIOCGNAME` | `0x010001`; the device's id; the name and its NUL, cut to the buffer |
| `EVIOCGPHYS`, `EVIOCGUNIQ` | `ENOENT` (virtual devices have neither) |
| `EVIOCGBIT(ev)`, `EVIOCGPROP` | the bitmap, `BITS_TO_LONGS(max) * 8` bytes at most (96 for keys); `EINVAL` for types evdev does not answer |
| `EVIOCGABS(axis)` | range and resolution, and the value; `EINVAL` on a device without axes |
| `EVIOCGKEY`, `EVIOCGLED`, `EVIOCGSND`, `EVIOCGSW`, `EVIOCGMTSLOTS` | the state as of the events this open file has read (see below) |
| `EVIOCGRAB` | on a control connection to the device: `0`, `EBUSY` while another open file (or this one) holds it, `EINVAL` releasing one not held |
| `EVIOCREVOKE` | shuts the connection: reads and requests `ENODEV`, poll hangs up |
| `EVIOCSCLOCKID` | `REALTIME`, `MONOTONIC`, `BOOTTIME`; else `EINVAL` |
| `EVIOCGREP`, `EVIOCSREP`, `EVIOCSFF`, `EVIOCRMFF` / `EVIOCGEFFECTS` | `ENOSYS` (no repeat, no force feedback) / 0 |
| other `'E'` requests | `EINVAL` |

The state requests answer from the descriptor's state updated by every
event the open file has read. That is the device's state minus what is still
queued, so it is consistent with the queue; Linux gets the same by dropping
queued events of the type it reports.

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
| `aim-host-display` unit tests | key map, modifiers, view-to-pixel mapping (Retina, resize, letterbox), input-core rules, packets, state for late openers, grab, injection, hotplug removal |
| `aim-linux-abi` `tests/input.rs` | the original `getevent -lpi` lists the three devices with their capabilities and held state (`BTN_TOUCH*`, `KEY_A*`), and `getevent -l` prints the packets `translate::Input` makes from a touch, a drag clamped at the edge, a key (AppKit repeat dropped), right Shift and a wheel line; `getevent -t` opens devices hotplugged under its inotify watch, with the event's own monotonic time |
| `tests/ndk.rs` `evdev` (`t_evdev.c`, Linux UAPI headers) | every ioctl above with Linux's return values and errnos, read/write/poll/epoll semantics, clocks, grab, revoke, stat/fstat/readdir/`/proc/self/fd`, `realpath` of `/sys/dev/char` |
| `tests/ndk.rs` `eventhub` (`t_eventhub.cpp`) | the image's `libinputreader.so` EventHub scans `/dev/input`, classifies `TOUCH \| TOUCH_MT`, `KEYBOARD \| ALPHAKEY` (built-in keyboard, from its `.idc`; `Generic.kl`, `Generic.kcm`) and `ROTARY_ENCODER`, finds sysfs root `/sys/devices/virtual`, and reads a key written into the keyboard |

A tap reaching an app needs system_server (P3) and is checked there.
`aim-display` itself was run: its nodes appear, `getevent -lp` lists them
with the window's size, and `SIGTERM` removes them. Its AppKit handlers were
not driven by real clicks and keys in these tests (that would post events to
the user's session); the translation they call is the one the tests drive.

## IME (#23)

Keys go out as physical keys, and Android's own IME composes text from
them. That is enough for Latin layouts and for Android's Korean, Japanese
and Chinese IMEs, which compose from key codes. Using the Mac's input
method (its candidate window, dictionaries and input-source switching,
`NSTextInputClient`) is a different path and would plug in like this:

- The content view adopts `NSTextInputClient` and sends `keyDown:` through
  `interpretKeyEvents:`. Keys the input method does not consume still go
  out as evdev keys.
- Marked text and committed text are text, not keys: they cannot travel as
  evdev events. They would go to the guest's focused editor through the
  input method framework: an Android IME service of ours (an APK in the
  derived image) receiving `setMarkedText`/`insertText` from the display
  server, and applying them with `InputConnection.setComposingText` and
  `commitText`. The window's side answers `firstRectForCharacterRange:`
  from the cursor rectangle that IME reports
  (`onUpdateCursorAnchorInfo`), mapped back from pixels to points.
- The evdev path here stays as it is; the IME is an addition beside it, not
  a replacement.
