package dev.darwinart.runtime.wm;

import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;

/** Narrow versioned Binder transport; no root lifecycle or Android focus policy. */
final class DesktopRootProtocol {
    static final String DESCRIPTOR = "dev.darwinart.runtime.wm.IDesktopRoot";
    private DesktopRootProtocol() {}
    static final class ServerIdentity {
        final int pid;
        final int uid;
        ServerIdentity(int pid, int uid) {
            if (pid <= 0 || uid < 0) throw new IllegalArgumentException("invalid root server identity");
            this.pid = pid;
            this.uid = uid;
        }
    }

    static ServerIdentity attachFocusDecisions(IBinder capability, IBinder callback)
            throws RemoteException {
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        try {
            data.writeInterfaceToken(DESCRIPTOR);
            data.writeInt(1);
            data.writeStrongBinder(callback);
            if (!capability.transact(IBinder.FIRST_CALL_TRANSACTION + 2, data, reply, 0))
                throw new RemoteException("desktop focus decision attachment rejected");
            reply.readException();
            return new ServerIdentity(reply.readInt(), reply.readInt());
        } finally {
            reply.recycle();
            data.recycle();
        }
    }
    /** Explicit server rejection, unlike a lost/uncertain transport ack. */
    static final class BindingRejectedException extends RemoteException {
        BindingRejectedException() { super("desktop window binding terminally rejected"); }
    }

    static IBinder register(IBinder service, long incarnation, IBinder lifetime)
            throws RemoteException {
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        try {
            data.writeInterfaceToken(DESCRIPTOR);
            data.writeLong(incarnation);
            data.writeStrongBinder(lifetime);
            if (!service.transact(IBinder.FIRST_CALL_TRANSACTION, data, reply, 0))
                throw new RemoteException("desktop root registration rejected");
            reply.readException();
            IBinder capability = reply.readStrongBinder();
            if (capability == null) throw new RemoteException("desktop root capability missing");
            return capability;
        } finally {
            reply.recycle();
            data.recycle();
        }
    }

    static void bindWindow(IBinder capability, IBinder originalChannel, int displayId)
            throws RemoteException {
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        try {
            data.writeInterfaceToken(DESCRIPTOR);
            data.writeInt(1); // BIND_WINDOW_V1: do not reinterpret REGISTER's body.
            data.writeStrongBinder(originalChannel);
            data.writeInt(displayId);
            if (!capability.transact(IBinder.FIRST_CALL_TRANSACTION + 1, data, reply, 0))
                throw new RemoteException("desktop window binding rejected");
            reply.readException();
            if (!reply.readBoolean()) throw new BindingRejectedException();
        } finally {
            reply.recycle();
            data.recycle();
        }
    }

    static void fact(IBinder capability, int kind, long incarnation, long serial, boolean key)
            throws RemoteException {
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        try {
            data.writeInterfaceToken(DESCRIPTOR);
            data.writeInt(kind);
            data.writeLong(incarnation);
            data.writeLong(serial);
            data.writeBoolean(key);
            if (!capability.transact(IBinder.FIRST_CALL_TRANSACTION, data, reply, 0))
                throw new RemoteException("desktop root fact rejected");
            reply.readException();
            if (!reply.readBoolean()) throw new RemoteException("desktop root fact was not retained");
        } finally {
            reply.recycle();
            data.recycle();
        }
    }
}
