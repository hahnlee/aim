package dev.aim.server;

import android.os.Binder;
import android.os.LocaleList;
import android.util.Slog;

import com.android.internal.app.LocalePicker;

/**
 * The device's languages, which follow the Mac's preferred languages
 * (docs/mac-settings.md): the service host hands the list over when the
 * bridge attaches and on each change of the Mac's, and it is applied as
 * Settings' language page applies a choice, persisted as the user's
 * (`system_locales`). A choice made in Android holds until the Mac's list
 * changes again.
 */
final class LocaleBridge {
    private static final String TAG = "AimLocaleBridge";

    private final Object mLock = new Object();
    /**
     * Whether PHASE_THIRD_PARTY_APPS_CAN_START has come. Until then the
     * list waits in mPending: where the bridge attaches
     * (PHASE_DEVICE_SPECIFIC_SERVICES_READY) AppOpsService is not ready and
     * ActivityManager refuses system_server's own change
     * (WRITE_SETTINGS), and until the apps' data is prepared
     * (waitForAppDataPrepared, right before this phase) a receiver of the
     * locale change could start without its data directory.
     */
    private boolean mReady;
    private String mPending;

    /** The host's list, `languageTags`: applied now, or once ready. */
    void update(String languageTags) {
        synchronized (mLock) {
            if (mReady) {
                apply(languageTags);
            } else {
                mPending = languageTags;
            }
        }
    }

    /** PHASE_THIRD_PARTY_APPS_CAN_START, before the persistent apps and home start. */
    void onThirdPartyAppsCanStart() {
        synchronized (mLock) {
            mReady = true;
            if (mPending != null) {
                apply(mPending);
                mPending = null;
            }
        }
    }

    /** LocalePicker.updateLocales with `languageTags`, as system_server. */
    private static void apply(String languageTags) {
        LocaleList locales = LocaleList.forLanguageTags(languageTags);
        long token = Binder.clearCallingIdentity();
        try {
            LocalePicker.updateLocales(locales);
        } finally {
            Binder.restoreCallingIdentity(token);
        }
        Slog.i(TAG, "languages set to the Mac's: " + locales.toLanguageTags());
    }
}
