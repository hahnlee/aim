package dev.darwinart.runtime.appops;

import android.os.IBinder;
import android.os.Parcel;

/** Focused host-Java tests for the Android 16 AppOps query wire contract. */
public final class AppOpsServiceEndpointTest {
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static Parcel request() {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(AppOpsServiceEndpoint.DESCRIPTOR);
        data.writeInt(67); // operation code
        data.writeInt(10042); // uid
        data.writeString("org.example.app");
        data.writeString(null);
        data.writeInt(0); // primary virtual device
        return data;
    }

    private static void testAllowedQuery() throws Exception {
        AppOpsServiceEndpoint endpoint = new AppOpsServiceEndpoint();
        Parcel reply = Parcel.obtain();
        check(endpoint.onTransact(AppOpsServiceEndpoint.TRANSACTION_CHECK_OPERATION_FOR_DEVICE,
                request(), reply, 0), "supported transaction returned false");
        check(reply.hasNoException(), "successful reply omitted writeNoException");
        check(reply.readInt() == AppOpsServiceEndpoint.MODE_ALLOWED,
                "AppOps query did not return MODE_ALLOWED");
        check(reply.dataAvail() == 0, "query reply contained trailing data");
    }

    private static void testValidation() throws Exception {
        AppOpsServiceEndpoint endpoint = new AppOpsServiceEndpoint();
        Parcel wrongToken = Parcel.obtain();
        wrongToken.writeInterfaceToken("not.com.android.internal.app.IAppOpsService");
        wrongToken.writeInt(67);
        wrongToken.writeInt(10042);
        wrongToken.writeString("org.example.app");
        wrongToken.writeString(null);
        wrongToken.writeInt(0);
        try {
            endpoint.onTransact(AppOpsServiceEndpoint.TRANSACTION_CHECK_OPERATION_FOR_DEVICE,
                    wrongToken, Parcel.obtain(), 0);
            throw new AssertionError("wrong interface token was accepted");
        } catch (SecurityException expected) {}

        Parcel trailing = request();
        trailing.writeInt(7);
        try {
            endpoint.onTransact(AppOpsServiceEndpoint.TRANSACTION_CHECK_OPERATION_FOR_DEVICE,
                    trailing, Parcel.obtain(), 0);
            throw new AssertionError("unexpected argument was accepted");
        } catch (IllegalStateException expected) {}
    }

    private static void testDescriptorAndUnsupported() throws Exception {
        AppOpsServiceEndpoint endpoint = new AppOpsServiceEndpoint();
        Parcel descriptorReply = Parcel.obtain();
        check(endpoint.onTransact(IBinder.INTERFACE_TRANSACTION, Parcel.obtain(), descriptorReply, 0),
                "interface transaction returned false");
        check(AppOpsServiceEndpoint.DESCRIPTOR.equals(descriptorReply.readString()),
                "interface descriptor changed");
        check(!endpoint.onTransact(AppOpsServiceEndpoint.TRANSACTION_CHECK_OPERATION_FOR_DEVICE + 1,
                request(), Parcel.obtain(), 0), "unsupported transaction was accepted");
    }

    public static void main(String[] args) throws Exception {
        check(AppOpsServiceEndpoint.TRANSACTION_CHECK_OPERATION_FOR_DEVICE == 55,
                "Android 16 transaction code changed");
        testAllowedQuery();
        testValidation();
        testDescriptorAndUnsupported();
        System.out.println("appops-endpoint: PASS");
    }
}
