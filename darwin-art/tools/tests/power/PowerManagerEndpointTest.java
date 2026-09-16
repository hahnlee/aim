package dev.darwinart.runtime.power;

import android.os.IBinder;
import android.os.Parcel;

public final class PowerManagerEndpointTest {
    private static final class FakePowerState implements PowerStateProvider {
        private int displayId = -1;

        @Override
        public boolean isInteractive() {
            return false;
        }

        @Override
        public boolean isDisplayInteractive(int displayId) {
            this.displayId = displayId;
            return displayId == 7;
        }
    }

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static Parcel transact(PowerManagerEndpoint endpoint, int code) throws Exception {
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        data.writeInterfaceToken(PowerManagerEndpoint.DESCRIPTOR);
        if (code == PowerManagerEndpoint.TRANSACTION_IS_DISPLAY_INTERACTIVE) {
            data.writeInt(7);
        }
        check(endpoint.onTransact(code, data, reply, 0), "transaction rejected: " + code);
        check(reply.hasNoException(), "successful reply omitted writeNoException");
        return reply;
    }

    public static void main(String[] args) throws Exception {
        FakePowerState fake = new FakePowerState();
        PowerManagerEndpoint endpoint = new PowerManagerEndpoint(fake);
        Parcel interactive = transact(endpoint, PowerManagerEndpoint.TRANSACTION_IS_INTERACTIVE);
        check(!interactive.readBoolean(), "provider state not used for isInteractive");

        Parcel displayInteractive = transact(endpoint,
                PowerManagerEndpoint.TRANSACTION_IS_DISPLAY_INTERACTIVE);
        check(displayInteractive.readBoolean(), "desktop display must be interactive");
        check(fake.displayId == 7, "display id was not forwarded to provider");

        Parcel descriptorData = Parcel.obtain();
        Parcel descriptorReply = Parcel.obtain();
        check(endpoint.onTransact(IBinder.INTERFACE_TRANSACTION, descriptorData,
                descriptorReply, 0), "descriptor transaction rejected");
        check(PowerManagerEndpoint.DESCRIPTOR.equals(descriptorReply.readString()),
                "wrong descriptor");

        Parcel wrongToken = Parcel.obtain();
        Parcel wrongReply = Parcel.obtain();
        wrongToken.writeInterfaceToken("not.android.os.IPowerManager");
        boolean rejected = false;
        try {
            endpoint.onTransact(PowerManagerEndpoint.TRANSACTION_IS_INTERACTIVE,
                    wrongToken, wrongReply, 0);
        } catch (SecurityException expected) {
            rejected = true;
        }
        check(rejected, "wrong interface token accepted");

        Parcel unsupportedData = Parcel.obtain();
        Parcel unsupportedReply = Parcel.obtain();
        unsupportedData.writeInterfaceToken(PowerManagerEndpoint.DESCRIPTOR);
        check(!endpoint.onTransact(IBinder.FIRST_CALL_TRANSACTION + 40,
                unsupportedData, unsupportedReply, 0), "unsupported call accepted");
        check(PowerManagerEndpoint.TRANSACTION_IS_INTERACTIVE == 21,
                "Android 16 isInteractive transaction drifted");
        check(PowerManagerEndpoint.TRANSACTION_IS_DISPLAY_INTERACTIVE == 22,
                "Android 16 isDisplayInteractive transaction drifted");
        System.out.println("power-manager-endpoint: PASS");
    }
}
