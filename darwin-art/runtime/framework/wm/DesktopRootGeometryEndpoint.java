package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;

/**
 * Private Binder transport between an application process's AppKit root and
 * the ActivityTask geometry owner. It carries host facts (content points of a
 * user resize, application results) in and task revisions out; it has no
 * geometry policy of its own.
 */
public final class DesktopRootGeometryEndpoint extends Binder {
    static final String DESCRIPTOR = "dev.darwinart.runtime.wm.IDesktopRootGeometry";
    static final int TRANSACTION_REGISTER = IBinder.FIRST_CALL_TRANSACTION;
    static final int TRANSACTION_REPORT = IBinder.FIRST_CALL_TRANSACTION + 1;
    static final int TRANSACTION_APPLIED = IBinder.FIRST_CALL_TRANSACTION + 2;

    private final TaskGeometryController tasks;

    public DesktopRootGeometryEndpoint(TaskGeometryController tasks) {
        if (tasks == null) throw new NullPointerException("tasks");
        this.tasks = tasks;
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code != TRANSACTION_REGISTER && code != TRANSACTION_REPORT
                && code != TRANSACTION_APPLIED) {
            return super.onTransact(code, data, reply, flags);
        }
        data.enforceInterface(DESCRIPTOR);
        int pid = Binder.getCallingPid();
        if (code == TRANSACTION_REGISTER) {
            IBinder receiver = data.readStrongBinder();
            int hostScale = data.readInt();
            int hostDisplay = data.readInt();
            data.enforceNoDataAvail();
            tasks.registerHost(pid, receiver, hostScale, hostDisplay);
            if (reply != null) reply.writeNoException();
            return true;
        }
        if (code == TRANSACTION_REPORT) {
            long serial = data.readLong();
            int pointsWidth = data.readInt();
            int pointsHeight = data.readInt();
            int hostScale = data.readInt();
            int hostDisplay = data.readInt();
            data.enforceNoDataAvail();
            tasks.hostResized(pid, serial, pointsWidth, pointsHeight, hostScale, hostDisplay);
            if (reply != null) reply.writeNoException();
            return true;
        }
        long revision = data.readLong();
        int status = data.readInt();
        data.enforceNoDataAvail();
        tasks.hostApplied(pid, revision, status);
        if (reply != null) reply.writeNoException();
        return true;
    }
}
