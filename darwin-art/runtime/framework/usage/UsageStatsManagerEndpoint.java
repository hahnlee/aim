package dev.darwinart.runtime.usage;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;

/**
 * Android 16 usage-stats service endpoint for the desktop profile.
 *
 * <p>The desktop policy has no Android app-idle scheduler. Every installed
 * application is therefore active, which is the platform's default bucket.
 * Keep this policy at the Android service boundary; no macOS provider is
 * needed for the default response.
 */
public final class UsageStatsManagerEndpoint extends Binder {
    // Android 16 IUsageStatsManager.aidl, verified against the shipped
    // _prebuilt/android-16/bootclasspath/framework.jar. The generated Stub
    // assigns getAppStandbyBucket to transaction 14.
    public static final String DESCRIPTOR = "android.app.usage.IUsageStatsManager";
    public static final int TRANSACTION_GET_APP_STANDBY_BUCKET =
            IBinder.FIRST_CALL_TRANSACTION + 13;
    public static final int STANDBY_BUCKET_ACTIVE = 10;

    public UsageStatsManagerEndpoint() {
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == INTERFACE_TRANSACTION) {
            if (reply != null) reply.writeString(DESCRIPTOR);
            return true;
        }
        if (code != TRANSACTION_GET_APP_STANDBY_BUCKET || reply == null) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }

        data.enforceInterface(DESCRIPTOR);
        data.readString(); // packageName; the desktop policy is package-independent.
        data.readString(); // callingAttributionTag; unused by the default policy.
        data.readInt(); // userId; this runtime exposes one Android user.
        data.enforceNoDataAvail();

        reply.writeNoException();
        reply.writeInt(STANDBY_BUCKET_ACTIVE);
        return true;
    }
}
