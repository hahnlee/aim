package dev.darwinart.runtime.os;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;

/** Borrowed endpoint on an IPC channel; process/channel lifetime belongs to its owner. */
public final class RemoteBinder extends Binder {
    public final int controlFd;
    public final int targetId;
    private boolean ready;

    public RemoteBinder(int controlFd, int targetId) {
        if (controlFd < 0 || targetId <= 0) throw new IllegalArgumentException("Invalid Binder endpoint");
        this.controlFd = controlFd;
        this.targetId = targetId;
    }

    public synchronized void awaitReady() {
        if (ready) return;
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        try {
            if (!nativeTransact(controlFd, targetId, IBinder.INTERFACE_TRANSACTION, data, reply, 0)) {
                throw new IllegalStateException("Remote Binder rejected descriptor query");
            }
            String descriptor = reply.readString();
            if (descriptor == null || descriptor.isEmpty()) {
                throw new IllegalStateException("Remote Binder has no descriptor");
            }
            attachInterface(null, descriptor);
            ready = true;
        } finally {
            reply.recycle();
            data.recycle();
        }
    }

    private static native boolean nativeTransact(
            int controlFd, int targetId, int code, Parcel data, Parcel reply, int flags);

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        return nativeTransact(controlFd, targetId, code, data, reply, flags);
    }
}
