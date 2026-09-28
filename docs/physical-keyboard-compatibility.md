# Physical keyboard compatibility

## Current state

Mac keys enter Android InputChannel/InputEventReceiver as an external
`SOURCE_KEYBOARD`, `KeyCharacterMap.FULL` device. Physical device 1 and virtual
fallback device -1 remain distinct. Timestamps use `CLOCK_MONOTONIC`;
NSEvent timestamps can drift by host sleep time and are not forwarded directly.

Pinned original AOSP KeyEvent/KeyCharacterMap evaluator, JNI and Parcel owners
replace the US-QWERTY switch and identity-only DAKM payload. SystemKeyboardMaps
selects original Generic/Virtual resources through guest filesystem access.
Empty-map semantics, modifiers/fallback, full Parcel round-trips, JNI allocation/
exception/ownership failures and both-flavor closure/warm gates pass.

Latest physical Calculator keyboard `4+5=9` passes after foreground-target tool
fixes. Chromium Ctrl+A historically inserted `a` despite correct transported
meta state; Shift+semicolon produced `:`. Original evaluator component tests
cover this contract, but full Chromium modifier/device-identity acceptance
remains open. Current app evidence is in
[architecture-migration.md](architecture-migration.md).

## Ownership contract

| Layer | Responsibility |
| --- | --- |
| Darwin provider | NSEvent/IOHID device facts, stable Android IDs, immutable host overlays, layout generations and hotplug lifetime; prefer Rust behind a narrow C ABI. |
| Original Android input owners | `.kl` scan-code/HID → key-code mapping; `.kcm` meta normalization, characters/dead keys/fallback/labels; JNI/Parcel map ownership. |
| KeyEvent / ViewRoot | Preserve device, scan/key code, source, meta, repeat and monotonic timestamps; deliver through the normal input channel. |
| IME | Composition/marked text belongs to InputMethodManager/InputConnection, separate from physical key-map character queries. |

Host TIS/UCKeyTranslate may build an immutable layout overlay when a generation
is created. Do not translate through an untracked process-global lookup at JNI
query time or grow a parallel Rust/C++ Android evaluator. Missing/malformed
resources fail visibly; Ctrl-only suppression is not a fix.

## Remaining work

1. Verify original JNI/device identity and Chromium modifier/fallback behavior
   physically on fresh current products.
2. Add stable descriptors and device-specific ANSI/ISO/JIS key-layout selection.
3. Compile macOS layout overlays covering Caps Lock, Option/Alt, AltGr-equivalent,
   dead keys and at least one non-US layout against Android semantics.
4. Publish host layout/hotplug changes as Android device generations and
   invalidate cached maps without app restart.
5. Implement marked text separately through InputConnection.

These broader keyboard items are follow-up scope beyond the bounded
production fixture-removal goal. Do not claim arbitrary host layouts or IME.

## Acceptance

- Character/label/number/match/fallback/event synthesis agree with the selected
  original map; physical FULL and virtual fallback identities remain distinct.
- Verify ANSI/ISO/JIS positions, modifiers/dead keys/non-US differential behavior
  and live generation changes.
- Unchanged Calculator, Calendar, DeskClock and editable TextView use production
  input without fixture shortcuts; composition uses InputConnection.
- Event clocks never regress relative to renderer time across host sleep/wake.

Source anchors: pinned original framework KeyCharacterMap JNI, native
`libs/input/KeyCharacterMap.cpp`, Java KeyCharacterMap and InputReader/EventHub.
Past implementation narratives and per-run diagnostics live in Git history.
