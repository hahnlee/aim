package dev.darwinart.runtime.input;

import android.os.Binder;
import android.os.Parcel;
import android.os.RemoteException;

/** System-process owner for the pinned Android 16 IInputManager contract. */
public final class InputManagerEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.hardware.input.IInputManager";
    private final int getVelocityTrackerStrategyCode = transaction("getVelocityTrackerStrategy");
    private final int getInputDeviceCode = transaction("getInputDevice");
    private final int getInputDeviceIdsCode = transaction("getInputDeviceIds");
    private final InputDeviceRegistry devices = new InputDeviceRegistry();

    public InputManagerEndpoint() {
        attachInterface(null, DESCRIPTOR);
    }

    private static int transaction(String name) {
        try {
            java.lang.reflect.Field field = Class.forName("android.hardware.input.IInputManager$Stub")
                    .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == getVelocityTrackerStrategyCode) {
            data.enforceInterface(DESCRIPTOR);
            data.enforceNoDataAvail();
            reply.writeNoException();
            // Null selects Android's built-in VelocityTracker strategy.
            reply.writeString(null);
            return true;
        }
        if (code == getInputDeviceCode) {
            data.enforceInterface(DESCRIPTOR);
            int id = data.readInt();
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeTypedObject(devices.inputDevice(id), 0);
            return true;
        }
        if (code == getInputDeviceIdsCode) {
            data.enforceInterface(DESCRIPTOR);
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeIntArray(devices.inputDeviceIds());
            return true;
        }
        return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
    }
}
