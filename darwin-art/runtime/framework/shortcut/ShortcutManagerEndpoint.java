package dev.darwinart.runtime.shortcut;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;

/** System-server endpoint for the Android 16 IShortcutService Binder contract. */
public final class ShortcutManagerEndpoint extends Binder {
    public static final String DESCRIPTOR = "android.content.pm.IShortcutService";

    // IShortcutService.Stub transaction ids in the pinned Android 16 framework.jar.
    public static final int TRANSACTION_REPORT_SHORTCUT_USED =
            IBinder.FIRST_CALL_TRANSACTION + 13;
    public static final int TRANSACTION_GET_SHORTCUTS =
            IBinder.FIRST_CALL_TRANSACTION + 22;

    private final ApplicationProcessRegistry processes;

    public ShortcutManagerEndpoint(ApplicationProcessRegistry applicationProcesses) {
        if (applicationProcesses == null) {
            throw new NullPointerException("applicationProcesses");
        }
        processes = applicationProcesses;
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == INTERFACE_TRANSACTION) {
            if (reply != null) reply.writeString(DESCRIPTOR);
            return true;
        }
        if (code != TRANSACTION_REPORT_SHORTCUT_USED
                && code != TRANSACTION_GET_SHORTCUTS) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }
        if (reply == null) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }

        data.enforceInterface(DESCRIPTOR);
        String packageName = data.readString();
        if (code == TRANSACTION_REPORT_SHORTCUT_USED) {
            data.readString(); // shortcutId: usage is advisory and needs no host-side persistence.
            data.readInt(); // userId: the current runtime exposes one Android user.
        } else {
            data.readInt(); // matchFlags: this endpoint has no persisted shortcut state.
            data.readInt(); // userId: the current runtime exposes one Android user.
        }
        data.enforceNoDataAvail();

        String callingPackage = processes.requireIdentifiedProcess(Binder.getCallingPid());
        if (packageName == null || !callingPackage.equals(packageName)) {
            throw new SecurityException("Shortcut caller does not own package " + packageName);
        }

        reply.writeNoException();
        if (code == TRANSACTION_GET_SHORTCUTS) {
            // ParceledListSlice is hidden from the SDK compile jar. Return the
            // framework-owned empty instance rather than defining a second
            // parcelable with the framework descriptor.
            reply.writeTypedObject(emptyParceledListSlice(),
                    Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
        }
        return true;
    }

    private static Parcelable emptyParceledListSlice() {
        try {
            return (Parcelable) Class.forName("android.content.pm.ParceledListSlice")
                    .getMethod("emptyList")
                    .invoke(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }
}
