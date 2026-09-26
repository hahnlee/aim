package android.telephony;

import android.content.Context;
import java.util.concurrent.Executor;

/**
 * Process-local telephony facade for the detached Android application runtime.
 *
 * The Darwin host has no modem or telephony system_server, but Android
 * applications still expect this framework object to exist when collecting
 * device metadata.  Keep the public contract deterministic and return the
 * same empty values Android uses when no SIM/operator is available.
 */
public final class TelephonyManager {
    public static final int PHONE_TYPE_NONE = 0;
    public static final int CALL_STATE_IDLE = 0;
    private final Context context;

    public TelephonyManager(Context context) {
        this.context = context;
    }

    public String getNetworkOperatorName() {
        return "";
    }

    public String getSimOperator() {
        return "";
    }

    public String getSimOperatorName() {
        return "";
    }

    public String getNetworkCountryIso() {
        return "";
    }

    public String getSimCountryIso() {
        return "";
    }

    public String getNetworkOperator() {
        return "";
    }

    public int getPhoneType() {
        return PHONE_TYPE_NONE;
    }

    public Context getContext() {
        return context;
    }

    /**
     * TelephonyRegistry reports a new callback's current state at once. With
     * no modem there is never a call, so the call state stays idle and no
     * later change is reported.
     */
    public void registerTelephonyCallback(Executor executor, TelephonyCallback callback) {
        if (executor == null || callback == null) {
            throw new IllegalArgumentException("executor and callback must be non-null");
        }
        if (callback instanceof TelephonyCallback.CallStateListener) {
            TelephonyCallback.CallStateListener listener =
                    (TelephonyCallback.CallStateListener) callback;
            executor.execute(() -> listener.onCallStateChanged(CALL_STATE_IDLE));
        }
    }

    public void unregisterTelephonyCallback(TelephonyCallback callback) {
        if (callback == null) throw new IllegalArgumentException("callback must be non-null");
    }
}
