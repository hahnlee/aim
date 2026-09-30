package dev.aim.mediaprojection;

import android.app.Activity;
import android.app.admin.DevicePolicyManager;
import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.os.Bundle;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.util.Log;

import dev.aim.server.IScreenCaptureConsent;
import dev.aim.server.IScreenCaptureConsentCallback;

/**
 * The activity MediaProjectionManager.createScreenCaptureIntent targets
 * (config_mediaProjectionPermissionDialogComponent), in place of SystemUI's
 * MediaProjectionPermissionActivity, with its result: RESULT_OK and the
 * app's IMediaProjection as EXTRA_MEDIA_PROJECTION once the user shares the
 * screen, else RESULT_CANCELED. It shows nothing itself: the native service
 * host asks the Mac, on the app's window, and creates the projection
 * through MediaProjectionManagerService as SystemUI does (docs/media.md).
 */
public final class ConsentActivity extends Activity {
    private static final String TAG = "AimMediaProjection";
    /** The native service host's consent service. */
    private static final String SERVICE = "aim.screen_capture_consent";
    /** IMediaProjectionManager.EXTRA_USER_REVIEW_GRANTED_CONSENT. */
    private static final String EXTRA_USER_REVIEW_GRANTED_CONSENT =
            "extra_media_projection_user_consent_required";
    /** IMediaProjectionManager.EXTRA_PACKAGE_REUSING_GRANTED_CONSENT. */
    private static final String EXTRA_PACKAGE_REUSING_GRANTED_CONSENT =
            "extra_media_projection_package_reusing_consent";
    /** MediaProjectionManager.EXTRA_MEDIA_PROJECTION. */
    private static final String EXTRA_MEDIA_PROJECTION =
            "android.media.projection.extra.EXTRA_MEDIA_PROJECTION";
    /** MediaProjectionManager.EXTRA_LAUNCH_COOKIE. */
    private static final String EXTRA_LAUNCH_COOKIE =
            "android.media.projection.extra.EXTRA_LAUNCH_COOKIE";

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        Intent launching = getIntent();
        boolean review = launching.getBooleanExtra(EXTRA_USER_REVIEW_GRANTED_CONSENT, false);
        String packageName = getLaunchedFromPackage();
        // Started without a result to return: only system_server's review of
        // a reused consent names the app, as SystemUI's activity accepts.
        if (getCallingPackage() == null) {
            if (!launching.hasExtra(EXTRA_PACKAGE_REUSING_GRANTED_CONSENT)) {
                cancel("not started for a result");
                return;
            }
            packageName = launching.getStringExtra(EXTRA_PACKAGE_REUSING_GRANTED_CONSENT);
        }
        ApplicationInfo info;
        try {
            info = getPackageManager().getApplicationInfo(packageName, 0);
        } catch (PackageManager.NameNotFoundException e) {
            cancel("no package " + packageName);
            return;
        }
        // A device policy that disables screen capture refuses it, as
        // SystemUI's ScreenCaptureDevicePolicyResolver does.
        if (getSystemService(DevicePolicyManager.class).getScreenCaptureDisabled(null)) {
            cancel("screen capture disabled by policy");
            return;
        }
        IBinder service = ServiceManager.getService(SERVICE);
        if (service == null) {
            cancel("no " + SERVICE);
            return;
        }
        try {
            IScreenCaptureConsent.Stub.asInterface(service).ask(packageName, info.uid,
                    label(info, packageName), review, launchCookie(launching),
                    new IScreenCaptureConsentCallback.Stub() {
                        @Override
                        public void onResult(IBinder projection) {
                            runOnUiThread(() -> answer(projection));
                        }
                    });
        } catch (RemoteException e) {
            cancel(SERVICE + ": " + e);
        }
    }

    /** The app's label up to its first line break, as SystemUI shows it. */
    private String label(ApplicationInfo info, String packageName) {
        String label = info.loadLabel(getPackageManager()).toString();
        for (int i = 0; i < label.length(); ) {
            int c = label.codePointAt(i);
            int type = Character.getType(c);
            if (type == Character.LINE_SEPARATOR || type == Character.CONTROL
                    || type == Character.PARAGRAPH_SEPARATOR) {
                label = label.substring(0, i) + "…";
                break;
            }
            i += Character.charCount(c);
        }
        return label.isEmpty() ? packageName : label;
    }

    /** The binder of the request's ActivityOptions.LaunchCookie, if any. */
    private static IBinder launchCookie(Intent launching) {
        Parcelable cookie = launching.getParcelableExtra(EXTRA_LAUNCH_COOKIE);
        if (cookie == null) {
            return null;
        }
        Parcel p = Parcel.obtain();
        try {
            cookie.writeToParcel(p, 0);
            p.setDataPosition(0);
            return p.readStrongBinder();
        } finally {
            p.recycle();
        }
    }

    private void answer(IBinder projection) {
        if (projection == null) {
            cancel("declined");
            return;
        }
        Bundle extras = new Bundle();
        extras.putBinder(EXTRA_MEDIA_PROJECTION, projection);
        Intent result = new Intent();
        result.putExtras(extras);
        setResult(RESULT_OK, result);
        finish();
    }

    private void cancel(String why) {
        Log.w(TAG, "screen capture not shared: " + why);
        setResult(RESULT_CANCELED);
        finish();
    }
}
