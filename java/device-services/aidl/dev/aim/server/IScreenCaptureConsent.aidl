package dev.aim.server;

import dev.aim.server.IScreenCaptureConsentCallback;

/**
 * The Mac's consent to an app's screen capture (docs/media.md): the media
 * bridge of the native service host registers it as
 * `aim.screen_capture_consent` for the consent activity
 * (java/screen-capture-consent) that config_mediaProjectionPermissionDialogComponent
 * names. It answers that activity's app only.
 */
oneway interface IScreenCaptureConsent {
    /**
     * What SystemUI's MediaProjectionPermissionActivity does once it knows
     * the app (`packageName`, of `uid`, labelled `label`): unless the app
     * may project already (hasProjectionPermission), asks the Mac, on the
     * app's window; then creates the projection (or reuses the granted one
     * when `reviewGrantedConsent`), gives it `launchCookie` if one, and
     * reports a reviewed consent. Tells `callback` the projection, or null
     * when the user declined.
     */
    void ask(String packageName, int uid, String label, boolean reviewGrantedConsent,
            @nullable IBinder launchCookie, IScreenCaptureConsentCallback callback);
}
