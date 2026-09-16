package dev.darwinart.runtime.trust;

import android.os.IBinder;
import android.os.Parcel;

/** Focused host-Java tests for the Android 16 trust-manager Binder contract. */
public final class TrustManagerEndpointTest {
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static Parcel request(int userId, int deviceId) {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(TrustManagerEndpoint.DESCRIPTOR);
        data.writeInt(userId);
        data.writeInt(deviceId);
        return data;
    }

    private static void testQuery(TrustManagerEndpoint endpoint, int transaction)
            throws Exception {
        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(transaction, request(0, 0), reply, 0),
                "supported transaction returned false: " + transaction);
        check(reply.hasNoException(), "successful reply omitted writeNoException");
        check(!reply.readBoolean(), "desktop profile reported a secured/locked device");
        check(reply.dataAvail() == 0, "query reply contained trailing data");
    }

    private static void testValidation(TrustManagerEndpoint endpoint) throws Exception {
        Parcel wrongToken = Parcel.obtain();
        wrongToken.writeInterfaceToken("not.android.app.trust.ITrustManager");
        wrongToken.writeInt(0);
        wrongToken.writeInt(0);
        try {
            endpoint.onTransact(TrustManagerEndpoint.TRANSACTION_IS_DEVICE_SECURE,
                    wrongToken, Parcel.obtain(), 0);
            throw new AssertionError("wrong interface token was accepted");
        } catch (SecurityException expected) {}

        Parcel trailing = request(0, 0);
        trailing.writeInt(7);
        try {
            endpoint.onTransact(TrustManagerEndpoint.TRANSACTION_IS_DEVICE_SECURE,
                    trailing, Parcel.obtain(), 0);
            throw new AssertionError("unexpected argument was accepted");
        } catch (IllegalStateException expected) {}
    }

    public static void main(String[] args) throws Exception {
        check(TrustManagerEndpoint.TRANSACTION_IS_DEVICE_LOCKED == 10,
                "Android 16 isDeviceLocked transaction changed");
        check(TrustManagerEndpoint.TRANSACTION_IS_DEVICE_SECURE == 11,
                "Android 16 isDeviceSecure transaction changed");
        TrustManagerEndpoint endpoint = new TrustManagerEndpoint();
        testQuery(endpoint, TrustManagerEndpoint.TRANSACTION_IS_DEVICE_LOCKED);
        testQuery(endpoint, TrustManagerEndpoint.TRANSACTION_IS_DEVICE_SECURE);

        Parcel descriptorReply = Parcel.obtain();
        check(endpoint.onTransact(IBinder.INTERFACE_TRANSACTION, Parcel.obtain(), descriptorReply, 0),
                "interface transaction returned false");
        check(TrustManagerEndpoint.DESCRIPTOR.equals(descriptorReply.readString()),
                "interface descriptor changed");
        check(!endpoint.onTransact(12, request(0, 0), Parcel.obtain(), 0),
                "unsupported transaction was accepted");
        testValidation(endpoint);
        System.out.println("trust-manager-endpoint: PASS");
    }
}
