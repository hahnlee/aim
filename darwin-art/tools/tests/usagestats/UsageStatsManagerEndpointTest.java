package dev.darwinart.runtime.usage;

import android.os.IBinder;
import android.os.Parcel;

/** Focused wire-contract test for the Android 16 usage-stats endpoint. */
public final class UsageStatsManagerEndpointTest {
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static Parcel request() {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(UsageStatsManagerEndpoint.DESCRIPTOR);
        data.writeString("com.example.app");
        data.writeString("com.example.app");
        data.writeInt(0);
        return data;
    }

    public static void main(String[] args) throws Exception {
        check(UsageStatsManagerEndpoint.TRANSACTION_GET_APP_STANDBY_BUCKET == 14,
                "Android 16 getAppStandbyBucket transaction drifted");
        check(UsageStatsManagerEndpoint.STANDBY_BUCKET_ACTIVE == 10,
                "Android ACTIVE standby bucket drifted");

        UsageStatsManagerEndpoint endpoint = new UsageStatsManagerEndpoint();
        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(UsageStatsManagerEndpoint.TRANSACTION_GET_APP_STANDBY_BUCKET,
                request(), reply, 0), "getAppStandbyBucket transaction rejected");
        check(reply.hasNoException(), "successful reply omitted writeNoException");
        check(reply.readInt() == UsageStatsManagerEndpoint.STANDBY_BUCKET_ACTIVE,
                "default policy must report ACTIVE");
        check(reply.dataAvail() == 0, "query reply contained trailing data");

        Parcel descriptorReply = Parcel.obtain();
        check(endpoint.onTransact(IBinder.INTERFACE_TRANSACTION, Parcel.obtain(), descriptorReply, 0),
                "interface transaction rejected");
        check(UsageStatsManagerEndpoint.DESCRIPTOR.equals(descriptorReply.readString()),
                "wrong interface descriptor");

        Parcel wrongToken = Parcel.obtain();
        wrongToken.writeInterfaceToken("not.android.app.usage.IUsageStatsManager");
        wrongToken.writeString("com.example.app");
        wrongToken.writeString("com.example.app");
        wrongToken.writeInt(0);
        try {
            endpoint.onTransact(UsageStatsManagerEndpoint.TRANSACTION_GET_APP_STANDBY_BUCKET,
                    wrongToken, Parcel.obtain(), 0);
            throw new AssertionError("wrong interface token was accepted");
        } catch (SecurityException expected) {}

        Parcel trailing = request();
        trailing.writeInt(7);
        try {
            endpoint.onTransact(UsageStatsManagerEndpoint.TRANSACTION_GET_APP_STANDBY_BUCKET,
                    trailing, Parcel.obtain(), 0);
            throw new AssertionError("unexpected argument was accepted");
        } catch (IllegalStateException expected) {}

        check(!endpoint.onTransact(IBinder.FIRST_CALL_TRANSACTION + 14,
                request(), Parcel.obtain(), 0), "unsupported transaction was accepted");
        System.out.println("usage-stats-endpoint: PASS");
    }
}
