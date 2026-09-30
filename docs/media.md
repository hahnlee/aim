# Media on the Mac: Now Playing and screen capture consent

Two things SystemUI does for media, done by the Mac while SystemUI still
runs (M1 of ADR 0013, [m1-shell.md](m1-shell.md), section 3 and step 3):
the playing media session is the Mac's Now Playing, of the app that plays,
and an app's request to capture the screen is answered by a Mac sheet on
the app's window. The original MediaSessionService and
MediaProjectionManagerService keep every session, projection and policy;
no framework code changes.

```text
 app's MediaSession ─▶ MediaSessionService ──IOnMediaKeyEventSessionChangedListener──▶ media bridge (guest-init)
                          ▲    │ ISessionController (metadata, state, callback)          │ NowPlaying / Gone
    play, pause, next,    │    └──────────────────────────────────────────────────────▶ │
    previous, seekTo      │                                                              ▼
                          └────────── Command ◀── shim (MPNowPlayingInfoCenter, MPRemoteCommandCenter)

 app: createScreenCaptureIntent ─▶ ConsentActivity (java/media-projection) ──IScreenCaptureConsent──▶ media bridge
                                        ▲ IMediaProjection                          │ Consent ▲ Consented
                                        └──── createProjection (MediaProjectionManagerService) ◀─┘ shim: sheet
```

## Pieces

| Piece | Where |
| --- | --- |
| The bridge: sessions, commands | `crates/aim-services/src/media/mod.rs` |
| `MediaMetadata`, `PlaybackState`, `MediaSession.Token` | `crates/aim-services/src/media/parcels.rs` |
| The consent service | `crates/aim-services/src/media/consent.rs`, `java/device-services/aidl` (`IScreenCaptureConsent`) |
| The consent activity | `java/media-projection` (`/system/app/AimMediaProjection`) |
| Its config | `java/framework-overlay`: `config_mediaProjectionPermissionDialogComponent` |
| Messages between bridge, server and shims | `crates/aim-host-display/src/media.rs` |
| Routing in the display server | `bin/aim-display/media.rs` |
| Now Playing, the sheet | `bin/aim-display/nowplaying.rs`, `bin/aim-display/consent.rs` |

guest-init starts the bridge with the binder host when it has a display
server (`--display`). It connects to the server (`OP_MEDIA`), opens a
binder process of its own (the system uid, system_server's context) and
registers `aim.screen_capture_consent` with servicemanager. All
transaction codes come from the pinned AIDL
(`crates/aim-services/sources.lock`): `ISessionManager`,
`IOnMediaKeyEventSessionChangedListener`, `ISessionController`,
`ISessionControllerCallback`, `IMediaProjectionManager`,
`IMediaProjection`, `IPackageManager.getPackagesForUid`.

In window mode the shims show Now Playing and the sheets; in device mode
the display server does, as the one Mac app.

## Now Playing

The session followed is the one Android's media keys go to: the bridge
registers `IOnMediaKeyEventSessionChangedListener` with
MediaSessionService (`addOnMediaKeyEventSessionChangedListener`, which the
system uid may call) and asks for the current one
(`getMediaKeyEventSession`). That is the session that played last
(`MediaSessionStack.getMediaButtonSession`), paused or not, so the Mac's
play key resumes what Android's would. Through the session's
`ISessionController` it reads the package, the metadata and the playback
state, and registers a controller callback as `MediaController` does. A
callback is a signal only: the bridge asks the session it follows for the
changed metadata or state, since a late callback may be of a session it
left. When MediaSessionService dies with system_server, Now Playing is
cleared and the bridge registers again.

| Android | Mac (`MPNowPlayingInfoCenter`) |
| --- | --- |
| `METADATA_KEY_DISPLAY_TITLE`, else `TITLE` | title |
| `ARTIST`, else `ALBUM_ARTIST`; `ALBUM` | artist, album |
| `DURATION` | playback duration |
| the state's position, advanced by its speed since its update time (`elapsedRealtime`, the host's `CLOCK_BOOTTIME`), within the duration | elapsed time, as of the message |
| speed while playing, fast-forwarding or rewinding, else 0 | playback rate |
| `ART`, else `ALBUM_ART`, else `DISPLAY_ICON` (bitmaps; ashmem ones are mapped) | artwork (`MPMediaItemArtwork`) |
| playing, fast-forwarding, rewinding; paused; none, stopped, error; buffering, connecting, skipping | playing; paused; stopped; interrupted |

The session goes to the shim that stands for its package, else the system
shim ("Android System"); a shim that does not run is opened in the
background (`--notifications`, no Dock icon until a window opens), since a
player may play with its window closed. A shim that loses the session
clears its Now Playing.

**Commands.** The shim enables the Mac's remote commands the state's
actions allow and sends each back as the controller's transport control,
as SystemUI's media controls do:

| Mac (`MPRemoteCommandCenter`) | Session (`ISessionController`) | Enabled by |
| --- | --- | --- |
| play, pause | `play`, `pause` | `ACTION_PLAY` or `ACTION_PLAY_PAUSE`; `ACTION_PAUSE` or `ACTION_PLAY_PAUSE` |
| toggle play/pause (the play key) | `pause` while playing, else `play` | either |
| stop | `stop` | `ACTION_STOP` |
| next, previous track | `next`, `previous` | `ACTION_SKIP_TO_NEXT`, `ACTION_SKIP_TO_PREVIOUS` |
| change playback position | `seekTo` | `ACTION_SEEK_TO` |

The Mac's media keys, the menu bar's Now Playing and Control Center send
these to the app it shows as playing.

## Screen capture consent

`MediaProjectionManager.createScreenCaptureIntent` starts
`config_mediaProjectionPermissionDialogComponent`, SystemUI's
`MediaProjectionPermissionActivity`, for a result. The device's static
overlay of framework-res names our activity there instead
(`dev.aim.mediaprojection/.ConsentActivity`, preinstalled in
`/system/app`, `forceQueryable`), in both modes: the activity must not
need SystemUI. It is invisible (translucent, in the app's task) and
returns SystemUI's result: `RESULT_OK` with the app's `IMediaProjection`
as `EXTRA_MEDIA_PROJECTION`, else `RESULT_CANCELED`.

Creating a projection needs `MANAGE_MEDIA_PROJECTION`, a signature
permission of the image's platform key, which our APK cannot have. So the
activity does what needs no permission, as SystemUI's does, and hands the
rest to the native service host, which has the system uid:

1. The activity takes the app from `getLaunchedFromPackage`; started
   without a result to return (`getCallingPackage` null), it accepts only
   system_server's review of a reused consent
   (`EXTRA_PACKAGE_REUSING_GRANTED_CONSENT`), else it cancels. It reads
   the app's uid and label (up to a line break) and
   `EXTRA_USER_REVIEW_GRANTED_CONSENT`, and asks
   `IScreenCaptureConsent`, found in servicemanager
   (`ServiceManager.getService`, a hidden API apps may use).
2. The host answers only the activity's app (`getPackagesForUid` of the
   caller). An app that may project already (`hasProjectionPermission`:
   `CAPTURE_VIDEO_OUTPUT`, or the `PROJECT_MEDIA` app op) is granted
   without a question, with its launch cookie if it gave one
   (`IMediaProjection.setLaunchCookie`), as SystemUI does for a
   system-privileged recorder.
3. Otherwise the display server asks on the app's window: its shim shows
   a sheet ("Share your screen with “App”?", Share screen, Cancel); when
   the server's own window shows the app's task (no shim of the app
   runs), the server does. A sheet with no window to attach to is an
   app-modal alert.
4. On Share the host creates the projection for the whole display
   (`createProjection(uid, package, TYPE_SCREEN_CAPTURE, false,
   DEFAULT_DISPLAY)`), or reuses the granted one when the user reviews a
   reused consent (`getProjection`), and then reports the review
   (`setUserReviewGrantedConsentResult`: `RECORD_CONTENT_DISPLAY`, or
   `RECORD_CANCEL` on Cancel), as SystemUI's
   `MediaProjectionServiceHelper` does. Requests still waiting when the
   display server goes away are cancelled.

SystemUI's single-app option (its app selector) is not offered: the whole
display is shared, which in window mode is every Android window. Its
device-policy check (screen capture disabled) and keyguard dismissal have
no counterpart: no device policy disables capture here and window mode
has no keyguard.

## Verification

Window-mode boots of fresh disposable data directories (2026-10-01),
reading the display server's and guest-init's log:

- `aim.screen_capture_consent` is in servicemanager, `cmd overlay lookup`
  gives `dev.aim.mediaprojection/dev.aim.mediaprojection.ConsentActivity`
  for `config_mediaProjectionPermissionDialogComponent`.
- VLC playing a local file: MediaSessionService's media button session is
  VLC's (`PLAYING`); the server logs `now playing org.videolan.vlc
  (Playing)`, VLC's shim publishes it, and Control Center queried
  mediaremoted for it. After `am force-stop`: `now playing nothing`.
- The consent activity started for VLC (system_server's reused-consent
  form, `EXTRA_PACKAGE_REUSING_GRANTED_CONSENT`): the sheet ("Share your
  screen with “VLC”?", Cancel / Share screen) on VLC's window, `consent 1
  asked` in both logs. Settings, which holds `CAPTURE_VIDEO_OUTPUT`, got
  its projection without a question.

Not exercised: the Mac's Now Playing UI itself and its commands (media
keys, Control Center's buttons), and an answer to the sheet: both need
clicks the check sessions could not make. The CTS modules
(CtsMediaProjectionTestCases, SDK33, SDK34; pinned in
`upstream/cts.lock`) click SystemUI's dialog with UiAutomator and wait for
it, so they fail or hang (#632).

Open: artwork given as a URI (#588), the single-app option (#589), a
localized sheet (#590).
