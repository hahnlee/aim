package dev.darwinart.runtime.power;

import android.os.IBinder;
import android.os.Parcel;

public final class ThermalServiceEndpointTest {
    private static final class StatusListener extends android.os.Binder {
        int callbackCount;
        int lastStatus = -1;

        @Override
        protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
                throws android.os.RemoteException {
            if (code != IBinder.FIRST_CALL_TRANSACTION) return false;
            data.enforceInterface("android.os.IThermalStatusListener");
            lastStatus = data.readInt();
            data.enforceNoDataAvail();
            callbackCount++;
            return true;
        }
    }

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static Parcel transact(ThermalServiceEndpoint endpoint, int code, int argument)
            throws Exception {
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        data.writeInterfaceToken(ThermalServiceEndpoint.DESCRIPTOR);
        if (argument != Integer.MIN_VALUE) data.writeInt(argument);
        check(endpoint.onTransact(code, data, reply, 0), "transaction rejected: " + code);
        check(reply.hasNoException(), "successful reply omitted writeNoException");
        return reply;
    }

    public static void main(String[] args) throws Exception {
        ThermalServiceEndpoint endpoint = new ThermalServiceEndpoint();

        Parcel status = transact(endpoint,
                ThermalServiceEndpoint.TRANSACTION_GET_CURRENT_THERMAL_STATUS,
                Integer.MIN_VALUE);
        check(status.readInt() == 0, "thermal status must be NONE");

        Parcel headroom = transact(endpoint,
                ThermalServiceEndpoint.TRANSACTION_GET_THERMAL_HEADROOM, 30);
        check(Float.isNaN(headroom.readFloat()), "headroom must report unavailable value");

        Parcel thresholds = transact(endpoint,
                ThermalServiceEndpoint.TRANSACTION_GET_THERMAL_HEADROOM_THRESHOLDS,
                Integer.MIN_VALUE);
        check(thresholds.readInt() == 7, "threshold array must cover statuses 0..6");
        for (int statusCode = 0; statusCode < 7; statusCode++) {
            check(Float.isNaN(thresholds.readFloat()), "threshold must be unavailable");
        }

        Parcel temperatures = transact(endpoint,
                ThermalServiceEndpoint.TRANSACTION_GET_CURRENT_TEMPERATURES,
                Integer.MIN_VALUE);
        check(temperatures.readInt() == 0, "temperature array must be empty");

        Parcel typedCooling = transact(endpoint,
                ThermalServiceEndpoint.TRANSACTION_GET_CURRENT_COOLING_DEVICES_WITH_TYPE, 1);
        check(typedCooling.readInt() == 0, "typed cooling array must be empty");

        StatusListener listener = new StatusListener();
        Parcel registerData = Parcel.obtain();
        Parcel registerReply = Parcel.obtain();
        registerData.writeInterfaceToken(ThermalServiceEndpoint.DESCRIPTOR);
        registerData.writeStrongBinder(listener);
        check(endpoint.onTransact(
                ThermalServiceEndpoint.TRANSACTION_REGISTER_THERMAL_STATUS_LISTENER,
                registerData, registerReply, 0), "status listener registration rejected");
        check(registerReply.hasNoException() && registerReply.readBoolean(),
                "status listener registration failed");
        check(listener.callbackCount == 1 && listener.lastStatus == 0,
                "new status listener did not receive THERMAL_STATUS_NONE");

        Parcel duplicateData = Parcel.obtain();
        Parcel duplicateReply = Parcel.obtain();
        duplicateData.writeInterfaceToken(ThermalServiceEndpoint.DESCRIPTOR);
        duplicateData.writeStrongBinder(listener);
        check(endpoint.onTransact(
                ThermalServiceEndpoint.TRANSACTION_REGISTER_THERMAL_STATUS_LISTENER,
                duplicateData, duplicateReply, 0), "duplicate registration transaction rejected");
        check(duplicateReply.hasNoException() && !duplicateReply.readBoolean(),
                "duplicate status listener accepted");

        Parcel unregisterData = Parcel.obtain();
        Parcel unregisterReply = Parcel.obtain();
        unregisterData.writeInterfaceToken(ThermalServiceEndpoint.DESCRIPTOR);
        unregisterData.writeStrongBinder(listener);
        check(endpoint.onTransact(
                ThermalServiceEndpoint.TRANSACTION_UNREGISTER_THERMAL_STATUS_LISTENER,
                unregisterData, unregisterReply, 0), "status listener removal rejected");
        check(unregisterReply.hasNoException() && unregisterReply.readBoolean(),
                "registered status listener was not removed");

        Parcel descriptorData = Parcel.obtain();
        Parcel descriptorReply = Parcel.obtain();
        check(endpoint.onTransact(IBinder.INTERFACE_TRANSACTION, descriptorData,
                descriptorReply, 0), "descriptor transaction rejected");
        check(ThermalServiceEndpoint.DESCRIPTOR.equals(descriptorReply.readString()),
                "wrong descriptor");

        Parcel wrongToken = Parcel.obtain();
        Parcel wrongReply = Parcel.obtain();
        wrongToken.writeInterfaceToken("not.android.os.IThermalService");
        boolean rejected = false;
        try {
            endpoint.onTransact(ThermalServiceEndpoint.TRANSACTION_GET_CURRENT_THERMAL_STATUS,
                    wrongToken, wrongReply, 0);
        } catch (SecurityException expected) {
            rejected = true;
        }
        check(rejected, "wrong interface token accepted");

        Parcel unsupportedData = Parcel.obtain();
        Parcel unsupportedReply = Parcel.obtain();
        unsupportedData.writeInterfaceToken(ThermalServiceEndpoint.DESCRIPTOR);
        check(!endpoint.onTransact(IBinder.FIRST_CALL_TRANSACTION + 40,
                unsupportedData, unsupportedReply, 0), "unknown call accepted");

        check(ThermalServiceEndpoint.TRANSACTION_GET_CURRENT_THERMAL_STATUS == 8,
                "Android 16 getCurrentThermalStatus transaction drifted");
        check(ThermalServiceEndpoint.TRANSACTION_GET_THERMAL_HEADROOM == 11,
                "Android 16 getThermalHeadroom transaction drifted");
        System.out.println("thermal-service-endpoint: PASS");
    }
}
