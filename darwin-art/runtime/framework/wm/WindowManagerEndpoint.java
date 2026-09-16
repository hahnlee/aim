package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.Parcel;
import android.os.RemoteException;

/** System-process owner for the pinned Android 16 IWindowManager contract. */
public final class WindowManagerEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.view.IWindowManager";
    private final WindowSessionEndpoint session;
    private final int openSessionCode = transaction("openSession");
    private final int hasNavigationBarCode = transaction("hasNavigationBar");

    public WindowManagerEndpoint(DesktopWindowMetadataRegistry metadata) {
        session = new WindowSessionEndpoint(metadata);
        attachInterface(null, DESCRIPTOR);
    }

    private static int transaction(String name) {
        try {
            java.lang.reflect.Field field = Class.forName("android.view.IWindowManager$Stub")
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
        if (code == openSessionCode) {
            data.enforceInterface(DESCRIPTOR);
            data.readStrongBinder(); // IWindowSessionCallback
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeStrongBinder(session);
            return true;
        }
        if (code != hasNavigationBarCode) return super.onTransact(code, data, reply, flags);
        data.enforceInterface(DESCRIPTOR);
        int displayId = data.readInt();
        data.enforceNoDataAvail();
        reply.writeNoException();
        // The Metal composer currently exposes no separate Android navigation
        // bar layer. Display 0 is therefore a decor-free built-in display.
        reply.writeBoolean(false);
        return true;
    }
}
