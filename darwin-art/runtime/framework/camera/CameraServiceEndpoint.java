package dev.darwinart.runtime.camera;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;

/**
 * Android camera-service boundary for hosts that have not published a camera provider yet.
 *
 * <p>The service exists and reports an empty device inventory. This is the same framework
 * contract as an Android device with no cameras and, unlike a missing {@code media.camera}
 * service, does not make {@code CameraManagerGlobal} retry the service-manager lookup forever.
 * A macOS AVFoundation provider can be added behind this Binder endpoint without changing app
 * processes or the service-manager contract.
 */
public final class CameraServiceEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.hardware.ICameraService";

    // android-16.0.0_r1 ICameraService.aidl transaction ordinals.
    private static final int GET_NUMBER_OF_CAMERAS = IBinder.FIRST_CALL_TRANSACTION;
    private static final int ADD_LISTENER = IBinder.FIRST_CALL_TRANSACTION + 4;
    private static final int GET_CONCURRENT_CAMERA_IDS = IBinder.FIRST_CALL_TRANSACTION + 5;
    private static final int REMOVE_LISTENER = IBinder.FIRST_CALL_TRANSACTION + 8;
    private static final int GET_VENDOR_TAG_DESCRIPTOR = IBinder.FIRST_CALL_TRANSACTION + 10;
    private static final int GET_VENDOR_TAG_CACHE = IBinder.FIRST_CALL_TRANSACTION + 11;
    private static final int IS_HIDDEN_PHYSICAL_CAMERA = IBinder.FIRST_CALL_TRANSACTION + 13;
    private static final int NOTIFY_SYSTEM_EVENT = IBinder.FIRST_CALL_TRANSACTION + 18;
    private static final int NOTIFY_DISPLAY_CONFIGURATION_CHANGE =
            IBinder.FIRST_CALL_TRANSACTION + 19;
    private static final int NOTIFY_DEVICE_STATE_CHANGE = IBinder.FIRST_CALL_TRANSACTION + 20;

    public CameraServiceEndpoint() {
        // A null local owner forces framework clients through the generated AIDL proxy, matching
        // the cross-process CameraService boundary used on Android.
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code < IBinder.FIRST_CALL_TRANSACTION) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }
        data.enforceInterface(DESCRIPTOR);
        switch (code) {
            case GET_NUMBER_OF_CAMERAS:
                requireReply(reply);
                reply.writeNoException();
                reply.writeInt(0);
                return true;
            case ADD_LISTENER:
                data.readStrongBinder();
                data.enforceNoDataAvail();
                requireReply(reply);
                reply.writeNoException();
                // Parcel.writeTypedArray(new CameraStatus[0], flags).
                reply.writeInt(0);
                return true;
            case GET_CONCURRENT_CAMERA_IDS:
                requireReply(reply);
                reply.writeNoException();
                // Parcel.writeTypedArray(new ConcurrentCameraIdCombination[0], flags).
                reply.writeInt(0);
                return true;
            case REMOVE_LISTENER:
                data.readStrongBinder();
                data.enforceNoDataAvail();
                requireReply(reply);
                reply.writeNoException();
                return true;
            case GET_VENDOR_TAG_DESCRIPTOR:
            case GET_VENDOR_TAG_CACHE:
                requireReply(reply);
                reply.writeNoException();
                // Parcel.writeTypedObject(null, flags).
                reply.writeInt(0);
                return true;
            case IS_HIDDEN_PHYSICAL_CAMERA:
                data.readString();
                data.enforceNoDataAvail();
                requireReply(reply);
                reply.writeNoException();
                reply.writeBoolean(false);
                return true;
            case NOTIFY_SYSTEM_EVENT:
            case NOTIFY_DISPLAY_CONFIGURATION_CHANGE:
            case NOTIFY_DEVICE_STATE_CHANGE:
                // CameraService treats these as one-way system notifications. With no provider
                // or devices there is no state to update.
                return true;
            default:
                return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }
    }

    private static void requireReply(Parcel reply) throws RemoteException {
        if (reply == null) throw new RemoteException("camera transaction requires a reply");
    }
}
