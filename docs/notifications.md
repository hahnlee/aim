# Notifications on the Mac (#4)

In window mode ([windows.md](windows.md)) Android's notifications are Mac
notifications of the app that posted them: its shim's banner, its entry in
Notification Center, its Dock badge. The original NotificationManagerService
(NMS) keeps every notification, channel, permission and policy, and SystemUI
keeps running; the bridge is one more of NMS's listeners, as SystemUI's is.
No APK or system app is added and no framework code changes (ADR 0012; M1
step 2 of [m1-shell.md](m1-shell.md)).

```text
 app ──enqueueNotification──▶ NMS (system_server) ──INotificationListener──▶ bridge (guest-init)
                                   ▲                                           │ Post / Remove
         sendIntentSender,         │                                           ▼
         onNotificationClick,      │                           display server (aim-display)
         cancelNotificationsFromListener                                       │ NOTIFY
                                   │                                           ▼
                                   └──── Click / Action / Dismiss ◀── shim (UNUserNotificationCenter)
```

## Pieces

| Piece | Where |
| --- | --- |
| The listener and what it does for the user's actions | `crates/aim-services/src/notifications/mod.rs` |
| `StatusBarNotification` and the other Java parcels | `crates/aim-services/src/notifications/parcels.rs` |
| Messages between bridge, server and shims | `crates/aim-host-display/src/notify.rs` |
| Routing in the display server, shim launches | `bin/aim-display/notifications.rs` |
| `UNUserNotificationCenter` in a shim | `bin/aim-display/un.rs` |
| The platform's shim, signed shims | `crates/aim-apps` (`app::system`, `shim::write`) |

## The listener

guest-init starts the bridge with the binder host when it has a display
server (`--display`). It connects to the server (`OP_NOTIFICATIONS`); in
device mode the server declines and the bridge stops, since SystemUI's shade
is on screen. In window mode it opens a binder process of its own (the
system uid, system_server's context, like the native services), waits for
`notification` in servicemanager and calls
`INotificationManager.registerListener(listener, {android,
aim.notifications.MacBridge}, USER_ALL)`, the call SystemUI's
`NotificationListener` makes: NMS records a system listener (no service is
bound) and sends it every user's notifications. It then reads what NMS
already holds (`getActiveNotificationsFromListener`). When NMS dies with
system_server, the Mac's notifications are removed and the bridge registers
again once NMS is back. All transaction codes come from the pinned AIDL
(`crates/aim-services/sources.lock`): `INotificationManager`,
`INotificationListener`, `IStatusBarNotificationHolder`,
`IStatusBarService`, `IActivityManager.sendIntentSender`.

NMS sends `onNotificationPosted(holder, rankingUpdate)`; the bridge fetches
the `StatusBarNotification` from the holder (`onNotificationPostedFull`
carries it directly when `android.app.no_sbnholder` is on). Both carry file
descriptors: the ranking update is shared memory
(`service.notification.ranking_update_ashmem`, on in the image), and a
bitmap over 12 KiB, or an icon's data over 16 KiB, is an ashmem blob. The
host's binder process accepts fds as libbinder does
(`aim_binder_host::local`); the bridge maps the blobs it reads and closes
the rest after the call.

What it reads of a notification (`Notification.writeToParcelImpl` at the
tag, in order): the allowlist token, content intent, ticker, large icon,
flags, group, extras (title, text, sub text, big text, conversation title,
inbox lines, messages with their senders, the big picture and large icon as
`Bitmap` or `Icon`), actions (title, `PendingIntent`, `RemoteInput`s),
channel and shortcut id. `RemoteViews` are not read: a notification with a
custom view keeps what precedes it (its ticker) (#468). Bitmaps are
RGBA, BGRA, RGB 565, alpha 8 or gray; icons given as PNG or other encoded
data are passed as they are; resource and URI icons are not drawn (#471).
Full-screen intents are not sent (#469).

## What the Mac shows

| Android | Mac |
| --- | --- |
| title (the conversation title first), sub text, big text or text (inbox lines; the last message, "sender: text") | title, subtitle, body |
| big picture, else large icon | an image attachment (PNG) |
| group (`overrideGroupKey`, else the group key) | thread identifier; group summaries are not shown (the Mac groups the thread) |
| channel importance MIN or LOW, or an update of an only-alert-once notification | passive: Notification Center only, no banner or sound |
| actions; a `RemoteInput` | buttons of a category; a text field |
| not ongoing, not a foreground service | counts toward the Dock badge |

The channel's importance comes from `getNotificationChannelForPackage`
(the ranking update's copy is in the shared memory, not parsed). What NMS
does not post (an app without `POST_NOTIFICATIONS`, a blocked app or
channel) never reaches the Mac. The Mac's Focus applies to the shims as to
any app; Android's own Do Not Disturb is not mapped to it.

## The shims

The server keeps what is shown. A notification goes to the host of the
shim that stands for its package: its primary one (of a package with
several launcher entries, the one named as the app); a package without a
shim (the shell, the phone app, the system) goes to the platform's shim,
"Android System" (`android`, written by aim-apps with framework-res's
label and icon, no activity). When that shim is not running, the server
opens it in the background (`open -g -j ... --args --notifications`): it
starts no activity and has no Dock icon until the app shows a window. A
shim that connects gets its package's
notifications; at start it removes what an earlier run left.

A shim posts with `UNUserNotificationCenter`: identifier the notification's
key, provisional authorization (no prompt; notifications go quietly to
Notification Center until the user lets the app show banners), a category
per set of actions with the custom dismiss action. macOS only lets a signed
bundle, outside temporary directories, post: aim-apps signs each shim ad hoc
(`codesign --sign -`). The Mac's per-app setting and Android's
`POST_NOTIFICATIONS` are independent (#470). After each post or removal the shim reports whether
the notification is delivered, and the server logs it (`aim-display:
notification KEY: shown`).

## What the user does

| On the Mac | The bridge |
| --- | --- |
| click | sends the content `PendingIntent` (`IActivityManager.sendIntentSender` with the notification's allowlist token and `MODE_BACKGROUND_ACTIVITY_START_ALLOWED`, as SystemUI does), then `IStatusBarService.onNotificationClick` (auto-cancel, statistics); the app's task comes to the front in its shim's window |
| an action | sends its `PendingIntent` |
| a reply | sends the action's `PendingIntent` with a fill-in intent carrying the text as `RemoteInput.addResultsToIntent` does, then `onNotificationDirectReplied` |
| dismiss | `cancelNotificationsFromListener` (NMS keeps ongoing ones) |

A notification Android removes is removed from Notification Center.

## Verification

A window-mode smoke boot (2026-09-30, `cargo aim boot --windows`, a
disposable data directory), reading the display server's log:

- The platform's notifications at first boot ("Android is starting",
  Play Store's) were shown by "Android System" and by Play Store's shim,
  launched in the background.
- `cmd notification post` run as the shell uid: plain, big picture with a
  data large icon (an ashmem blob over 16 KiB) and messaging style
  notifications shown by "Android System" (`shown`).
- A Clock timer (`am start -a android.intent.action.SET_TIMER ...
  SKIP_UI`): shown by Clock's shim, titled "Clock" (its content is a
  custom view, #468); a click (as the shim sends it) sent its content
  intent and `DeskClock` became the resumed activity (its task goes to
  Clock's shim, which was running).
- A dismissal (as the shim sends it) cancelled the notification in NMS
  (`cmd notification list`), which then left Notification Center (`not
  shown`).
- Settings cold-starts (`am start -W`) as before.

Not exercised end to end: a click on the Mac itself (the shim's
`UNUserNotificationCenter` delegate), actions and replies (their
`PendingIntent`s go through the same `sendIntentSender` as clicks; the
reply's fill-in intent is checked against the `Intent` reader in unit
tests).

**CTS** (`CtsNotificationTestCases` of CTS 16_r1, pinned in
`upstream/cts.lock`; installed with its helper APKs as its
`AndroidTest.xml` lists them, run with `am instrument` as in
[system-services.md](system-services.md), window mode):

| Class | Tests | Without the bridge (main) | With the bridge |
| --- | --- | --- | --- |
| NotificationManagerTest (listeners, channels, styles, trampolines, autogrouping, ...) | 114 | 114 pass | 114 pass |
| StatusBarNotificationTest | 20 | 20 pass | 20 pass |
| NotificationStatsTest | 11 | 11 pass | 11 pass |
