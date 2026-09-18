# Physical keyboard compatibility

## Status and decision

Darwin ART exposes the Mac keyboard to applications as an external Android
`InputDevice` with `SOURCE_KEYBOARD` and `KeyCharacterMap.FULL`. AppKit key
events enter the same `InputChannel`/`InputEventReceiver` path used by Android
window input. Device id `-1` remains the Android virtual-keyboard fallback; it
does not represent the host keyboard.

Input event timestamps are sampled from `CLOCK_MONOTONIC`, matching Android's
InputReader clock and Chromium's `base::TimeTicks`. `NSEvent.timestamp` is not
forwarded directly: on macOS it can differ from that clock by accumulated
system-sleep time, which makes a current event appear older than a renderer's
document time origin.

The original hard-coded US-QWERTY evaluator and identity-only DAKM Parcel
payload have now been removed from the source tree. Framework input registration
delegates to pinned original KeyEvent/KeyCharacterMap JNI; SystemKeyboardMaps
selects original Generic/Virtual resources through the guest filesystem factory.
Both production flavors linked successfully after the allocation-handoff fix;
source/link fixture boundaries and exact warm no-work also pass.
A fresh normal Calculator generation physically computes 2+3=5 and accepts
hardware 2,3,Return as 23 through the original-map path before that fix. Actual
original JNI null/pending-exception allocation, transfer/delete and bad_alloc
tests pass with genuine production providers. Post-guard APK acceptance, device
identity and Chromium modifier interaction remain unverified.

The table was introduced as a bootstrap compatibility bridge so unmodified
AOSP applications could accept physical-keyboard input while the common event
pipeline was brought up. It is not the target architecture and must not grow
into a second keyboard-layout implementation one switch case at a time.

Physical Chromium testing on 2026-09-18 proves a concrete modifier defect:
CGHID Control+A arrives as Android key code 29 with meta state 4096 but inserts
`a`; Shift+semicolon arrives with meta state 1 and correctly produces `:`.
The transport preserves these modifiers; the replacement evaluator does not.
That pre-cutover implementation also fabricated a FULL map for `obtainEmptyMap`
and parceled device identity instead of map contents. Original evaluator/Parcel
component execution now passes exact modifiers, fallback and full payload
round-trips, but original JNI/device identity and physical acceptance remain open.

The bounded correction is pinned original AOSP evaluator, JNI and Parcel
ownership, with genuine Generic and Virtual resources selected by the system
input-device owner. Physical device 1 and virtual device -1 remain distinct;
restore original empty-map semantics. Missing/malformed resources fail visibly.
A Ctrl-only suppression branch is not a fix: original matching also owns
alternate characters and fallback actions. This adoption does not claim
arbitrary macOS layout, hotplug or IME support.

## Android contract

Android separates physical keys from produced text:

1. A device-specific key layout (`.kl`) maps Linux scan codes or HID usages to
   Android key codes.
2. A device-specific key character map (`.kcm`) maps an Android key code plus a
   normalized meta state to characters, dead keys, fallback actions, display
   labels, and numeric labels.
3. `KeyEvent` preserves the physical event identity: device id, scan code, key
   code, source, meta state, repeat count, and timestamps.
4. `KeyCharacterMap.nativeGetCharacter()` crosses JNI and delegates to the
   native per-device `KeyCharacterMap`; Java applies Android's combining-accent
   conventions where required.

This is distinct from IME composition. An IME edits text through
`InputMethodManager` and `InputConnection`; it must not be simulated by
returning composed strings from `KeyMapGetCharacter`.

Authoritative AOSP implementations:

- [Framework JNI bridge](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/jni/android_view_KeyCharacterMap.cpp)
- [Native KeyCharacterMap](https://android.googlesource.com/platform/frameworks/native/+/refs/heads/main/libs/input/KeyCharacterMap.cpp)
- [Java KeyCharacterMap contract](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/view/KeyCharacterMap.java)
- [InputReader/EventHub device path](https://android.googlesource.com/platform/frameworks/native/+/refs/heads/main/services/inputflinger/reader/EventHub.cpp)

## Observable compatibility gaps

These gaps were observed in the pre-cutover implementation. Original evaluator
component tests now cover map queries, modifier matching and full Parcel payload,
but product-level coverage is not yet complete. Host layout, hotplug and IME
items remain separate follow-up work:

- The active macOS layout and actual keyboard device do not affect the fixed
  US-QWERTY character table.
- Caps Lock, Option/Alt, AltGr, Control combinations, dead keys, compose
  sequences, and non-US layouts are incomplete.
- ANSI, ISO, and JIS physical key positions are not represented by distinct
  device layouts.
- `getDisplayLabel`, `getNumber`, `getMatch`, `getFallbackAction`, and
  `getEvents` do not yet form a coherent per-device map.
- Keyboard hotplug and host input-source changes do not publish a new Android
  input-device generation or invalidate a cached map.
- Editable-text composition and marked text do not yet have a complete
  `InputConnection` path.

## Target ownership

```text
NSEvent and IOHID device identity
              |
              v
Darwin input device registry (Rust)
  - stable Android device ids and generations
  - immutable physical layout (.kl-equivalent)
  - immutable character map (.kcm-equivalent)
              |
              v
versioned, table-oriented C ABI
              |
              v
Android InputDevice / KeyCharacterMap / KeyEvent
              |
              v
InputChannel -> ViewRootImpl -> application
```

Rust should own host device identity, immutable host-overlay data, cache lifetime,
layout generations, and hotplug transitions. Original Android map parsing,
evaluation and serialization stay with pinned AOSP owners, not a parallel Rust
reimplementation. AppKit/Carbon adapters may obtain the
active macOS input source and use `UCKeyTranslate` to compile a deterministic
host-layout overlay. JNI C++ retains original Android map ownership and queries;
it must not own a parallel mutable host keyboard model.

The runtime should parse Android `.kl` and `.kcm` data so Android's generic and
device-specific maps remain the semantic baseline. A host-generated overlay
may provide characters for the selected macOS layout, but it must preserve
Android key codes, meta-state normalization, dead-key representation, and
fallback behavior. Host translation should occur when a map generation is
built, not as an untracked process-global lookup during an arbitrary JNI call.

## Delivery milestones

1. Replace the C++ switch and identity-only Parcel bridge with pinned original
   AOSP evaluator/JNI/serialization. Package original Generic/Virtual maps and
   select them explicitly at the system input-device owner; preserve empty maps.
2. Extend device-specific map selection using stable input-device descriptors,
   retaining original Android `.kl` and `.kcm` owners.
3. Compile macOS TIS/`UCKeyTranslate` layouts into overlays; cover ANSI, ISO,
   JIS, Shift, Caps Lock, Option/Alt, AltGr-equivalent, and dead keys.
4. Publish device hotplug and layout changes with Android-style generations
   and cache invalidation.
5. Implement marked-text and composition separately through the Android
   `InputMethodManager`/`InputConnection` boundary.

## Completion criteria

The broader keyboard follow-up (not the current bounded Probe-removal goal)
is complete only when all of the following hold:

- `KeyMapGetCharacter` contains no hard-coded US-layout switch.
- Physical keyboard events remain external `FULL` keyboard events; the
  virtual device id is used only as Android's fallback map.
- Character, display-label, number, match, fallback-action, and event-synthesis
  queries agree with the selected Android character map.
- Shift, Caps Lock, Option/Alt, dead-key composition, and at least one non-US
  layout pass differential tests against AOSP behavior.
- ANSI, ISO, and JIS devices have verified scan-code/key-code position tests.
- A host layout change is reflected without restarting the application, via a
  defined input-device generation change.
- Unmodified AOSP Calculator, Calendar, DeskClock, and an editable `TextView`
  pass through the production input path without fixture-specific shortcuts.
- IME composition tests pass through `InputConnection` rather than extending
  the physical-key character-map bridge.
- Pointer and key event times stay in Android's monotonic clock domain across
  host sleep/wake and never regress relative to renderer/document time.
