package dev.aim.server;

/** The answer to IScreenCaptureConsent.ask. */
oneway interface IScreenCaptureConsentCallback {
    /** The app's IMediaProjection, or null: the user declined. */
    void onResult(@nullable IBinder projection);
}
