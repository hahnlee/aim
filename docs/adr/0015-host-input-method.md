# ADR 0015: The Mac's input method is the app host's input method

Status: accepted 2026-11-04 (#23). Records the owner's choices of
2026-10-17 (the Mac's input method, no Android IME) and 2026-10-19 (empty
input method lists) under ADR 0013 ("Services with a macOS owner") and
ADR 0014.

## Context

On Android an input method is an app: an `InputMethodService` that
InputMethodManagerService binds while an editor has the focus, and that
types into the editor through its `InputConnection`
(`IRemoteInputConnection` across processes). The original image's Gboard
composes Korean, Japanese and Chinese from physical keys, but its
candidates, dictionaries and switching are not the Mac's, and the Mac's
own input method (its input sources, candidate window, dictation and
emoji picker) never sees a key that goes out as an evdev key
([input.md](../input.md), "IME").

Two ways put the Mac's input method in front of Android editors. A
bridging IME APK in the derived image (what ChromeOS's ARC and Windows
Subsystem for Android do) keeps InputMethodManagerService original but
adds a new system component, an app with the one framework class an IME
must extend, and a protocol between it and the host. In the app host
InputMethodManager is already native (ADR 0013), and the editor's input
connection reaches it in `startInputOrWindowGainedFocus`, so the service
can itself make an input method's calls on it.

## Decision

- **The host is the input method provider; no Android IME runs.** To
  Android the device has a physical keyboard and no input method. The app
  host's native InputMethodManager (`aim_services::app_host::input_method`)
  answers the input method lists empty whatever is installed, and no
  `InputMethodService` is bound. There is no IME APK in the image.
- **An editor's start is the host's input start.** When
  `startInputOrWindowGainedFocusAsync` carries an `EditorInfo` and an
  input connection, InputMethodManager keeps the editor's
  `IRemoteInputConnection`, tells the display server (the text input
  connection, `aim_host_display::text_input`) which task has the editor
  and its `inputType` and `imeOptions`, and answers the client
  `InputBindResult.NO_IME`, so an app's `invalidateInput` restarts its
  input with a new connection. A start without an editor, or the app's
  next start, ends it.
- **The window showing the editor is the Mac's input method's client.**
  Only while its task has an editor, that window's content view sends its
  keys without Command or Control through `interpretKeyEvents:` as an
  `NSTextInputClient`. Marked text becomes `setComposingText` (plain text:
  the editor's default composing spans underline it), inserted text
  `commitText`, a replacement of the text it typed
  `deleteSurroundingText` first, `unmarkText` `finishComposingText`, and
  the editing commands what an input method sends (Return the editor's
  action as `InputMethodService.sendDefaultEditorAction` does, Tab, Esc
  and the arrows as soft keyboard keys). The calls are oneway, in the
  connection's session, as `InputConnection` makes them from an IME.
- **When the editor goes, the composition goes.** A new start or an end
  makes the app's InputMethodManager finish the old connection's
  composing text (`RemoteInputConnectionImpl.deactivate`), so the view
  discards the input method's marked text; what the input method gives
  in answer, in the call or later from its process until it takes the
  next key, is the old editor's and changes nothing.
- **Everything else keeps the raw key path.** Keys outside an editor (a
  game, a list, a non-editor `View`) and keys with Command or Control go
  out as evdev keys through EventHub as before (a release goes where its
  press went), so games and shortcuts still get key events.
- **The original image is unchanged.** Its Gboard keeps composing from
  physical keys; the host input method is the app host's.

## Consequences

- Every Mac input source, its candidates, dictation and the emoji picker
  type into Android editors, and the Mac's input-source switching works as
  in any Mac app. Android's IME settings show no keyboard.
- `InputMethodManager.showSoftInput` and `hideSoftInput` report that no
  input method shows or hides; apps that wait for an IME window's insets
  get none, as on a device with a physical keyboard.
- The candidate window sits at the window's bottom left until the editor's
  `CursorAnchorInfo` places it at the caret (#1458), in points from the
  editor's pixels. The view reads back only the text the input method
  typed since the caret last moved, not the editor's own text and
  selection (`updateSelection` is not fed back yet).
- An app that needs a real `InputMethodService` (an IME's own settings, a
  test of one) has none to bind.
