package dev.darwinart.runtime.os;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;

/** Borrowed endpoint on an IPC channel; process/channel lifetime belongs to its owner. */
public final class RemoteBinder extends Binder {
    public final int controlFd;
    public final int targetId;
    public final long channelGeneration;
    private boolean ready;

    public RemoteBinder(int controlFd, int targetId, long channelGeneration) {
        if (controlFd < 0 || targetId <= 0 || channelGeneration == 0)
            throw new IllegalArgumentException("Invalid Binder endpoint");
        this.controlFd = controlFd;
        this.targetId = targetId;
        this.channelGeneration = channelGeneration;
    }

    public synchronized void awaitReady() {
        if (ready) return;
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        try {
            if (!nativeTransact(controlFd, channelGeneration, targetId, IBinder.INTERFACE_TRANSACTION, data, reply, 0)) {
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
            int controlFd, long channelGeneration, int targetId, int code, Parcel data, Parcel reply, int flags);

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        return nativeTransact(controlFd, channelGeneration, targetId, code, data, reply, flags);
    }
}
