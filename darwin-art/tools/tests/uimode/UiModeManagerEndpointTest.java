package dev.darwinart.runtime.uimode;

import android.os.IBinder;
import android.os.Parcel;

public final class UiModeManagerEndpointTest {
    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static Parcel transact(UiModeManagerEndpoint endpoint, int code) throws Exception {
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        data.writeInterfaceToken(UiModeManagerEndpoint.DESCRIPTOR);
        check(endpoint.onTransact(code, data, reply, 0), "transaction rejected: " + code);
        check(reply.hasNoException(), "successful reply omitted writeNoException");
        return reply;
    }

    public static void main(String[] args) throws Exception {
        UiModeManagerEndpoint endpoint = new UiModeManagerEndpoint();
        Parcel contrast = transact(endpoint, UiModeManagerEndpoint.TRANSACTION_GET_CONTRAST);
        check(contrast.readFloat() == 0.0f, "default contrast must be normal");

        Parcel mode = transact(endpoint,
                UiModeManagerEndpoint.TRANSACTION_GET_CURRENT_MODE_TYPE);
        check(mode.readInt() == 1, "desktop profile must expose normal UI mode");

        Parcel night = transact(endpoint, UiModeManagerEndpoint.TRANSACTION_GET_NIGHT_MODE);
        check(night.readInt() == 1, "initial night mode must be MODE_NIGHT_NO");

        Parcel descriptorData = Parcel.obtain();
        Parcel descriptorReply = Parcel.obtain();
        check(endpoint.onTransact(IBinder.INTERFACE_TRANSACTION, descriptorData,
                descriptorReply, 0), "descriptor transaction rejected");
        check(UiModeManagerEndpoint.DESCRIPTOR.equals(descriptorReply.readString()),
                "wrong descriptor");

        Parcel unsupportedData = Parcel.obtain();
        Parcel unsupportedReply = Parcel.obtain();
        unsupportedData.writeInterfaceToken(UiModeManagerEndpoint.DESCRIPTOR);
        check(!endpoint.onTransact(IBinder.FIRST_CALL_TRANSACTION + 40,
                unsupportedData, unsupportedReply, 0), "unsupported call accepted");
        check(UiModeManagerEndpoint.TRANSACTION_GET_CONTRAST == 27,
                "Android 16 getContrast transaction drifted");
        System.out.println("uimode-endpoint: PASS");
    }
}
