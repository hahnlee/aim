package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;

/** Private Binder registration boundary for the app-process desktop provider. */
public final class DesktopWindowMetadataEndpoint extends Binder {
    static final String DESCRIPTOR = "dev.darwinart.runtime.wm.IDesktopWindowMetadata";
    static final int TRANSACTION_REGISTER = IBinder.FIRST_CALL_TRANSACTION;
    private final ApplicationProcessRegistry applications;
    private final DesktopWindowMetadataRegistry metadata;

    public DesktopWindowMetadataEndpoint(ApplicationProcessRegistry processes,
            DesktopWindowMetadataRegistry registry) {
        applications = processes;
        metadata = registry;
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code != TRANSACTION_REGISTER || reply == null) return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        data.enforceInterface(DESCRIPTOR);
        IBinder receiver = data.readStrongBinder();
        data.enforceNoDataAvail();
        int pid = Binder.getCallingPid();
        applications.requireIdentifiedProcess(pid);
        metadata.register(pid, receiver);
        reply.writeNoException();
        return true;
    }
}
