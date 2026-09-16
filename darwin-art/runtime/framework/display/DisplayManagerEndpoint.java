package dev.darwinart.runtime.display;

import android.os.Binder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import java.lang.reflect.Method;

/** System-process owner for the pinned Android 16 IDisplayManager contract. */
public final class DisplayManagerEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.hardware.display.IDisplayManager";
    private final DefaultDisplayRegistry displays = new DefaultDisplayRegistry();
    private final int getDisplayInfoCode = transaction("getDisplayInfo");
    private final int getDisplayIdsCode = transaction("getDisplayIds");
    private final int isUidPresentOnDisplayCode = transaction("isUidPresentOnDisplay");
    private final int preferredWideGamutCode = transaction("getPreferredWideGamutColorSpaceId");
    private final int overlaySupportCode = transaction("getOverlaySupport");

    public DisplayManagerEndpoint() {
        attachInterface(null, DESCRIPTOR);
    }

    private static int transaction(String name) {
        try {
            java.lang.reflect.Field field =
                    Class.forName("android.hardware.display.IDisplayManager$Stub")
                            .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    private static Parcelable createOverlayProperties() {
        try {
            Class<?> type = Class.forName("android.hardware.OverlayProperties");
            Method getDefault = type.getDeclaredMethod("getDefault");
            getDefault.setAccessible(true);
            return (Parcelable) getDefault.invoke(null);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Android OverlayProperties ABI mismatch", error);
        }
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == getDisplayInfoCode) {
            data.enforceInterface(DESCRIPTOR);
            int displayId = data.readInt();
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeTypedObject(
                    displays.getDisplayInfo(displayId),
                    Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code == getDisplayIdsCode) {
            data.enforceInterface(DESCRIPTOR);
            boolean includeDisabled = data.readBoolean();
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeIntArray(displays.getDisplayIds(includeDisabled));
            return true;
        }
        if (code == isUidPresentOnDisplayCode) {
            data.enforceInterface(DESCRIPTOR);
            data.readInt(); // uid; visibility policy is added with virtual displays.
            int displayId = data.readInt();
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeBoolean(displayId == 0);
            return true;
        }
        if (code != preferredWideGamutCode && code != overlaySupportCode) {
            return super.onTransact(code, data, reply, flags);
        }
        data.enforceInterface(DESCRIPTOR);
        data.enforceNoDataAvail();
        reply.writeNoException();
        if (code == preferredWideGamutCode) {
            // ColorSpace.Named.SRGB. The Metal display backend does not yet
            // publish a host wide-gamut mode through Android DisplayManager.
            reply.writeInt(0);
        } else {
            // No dedicated HWC overlay plane: SurfaceFlinger composites the
            // Android layer tree through the Metal backend. A non-null default
            // object is still mandatory: HardwareRenderer asks it whether an
            // RGBA buffer/dataspace pair can use an overlay during process
            // initialization.
            reply.writeTypedObject(
                    createOverlayProperties(), Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
        }
        return true;
    }
}
