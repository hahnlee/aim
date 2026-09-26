package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;

/** Binder service and per-registration desktop-root capability endpoint. */
public final class DesktopRootEndpoint extends Binder {
    static final String DESCRIPTOR = "dev.darwinart.runtime.wm.IDesktopRoot";
    static final int TRANSACTION_REGISTER = IBinder.FIRST_CALL_TRANSACTION;
    static final int TRANSACTION_FACT = IBinder.FIRST_CALL_TRANSACTION;
    static final int TRANSACTION_BIND_WINDOW = IBinder.FIRST_CALL_TRANSACTION + 1;
    static final int TRANSACTION_ATTACH_FOCUS_DECISIONS = IBinder.FIRST_CALL_TRANSACTION + 2;

    private final ApplicationProcessRegistry applications;
    private final DesktopRootRegistry roots;

    public DesktopRootEndpoint(ApplicationProcessRegistry processes, DesktopRootRegistry registry) {
        if (processes == null || registry == null) throw new NullPointerException();
        applications = processes;
        roots = registry;
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code != TRANSACTION_REGISTER || reply == null) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }
        data.enforceInterface(DESCRIPTOR);
        long incarnation = data.readLong();
        IBinder clientLifetime = data.readStrongBinder();
        data.enforceNoDataAvail();

        WindowSessionIdentity identity = new WindowSessionIdentity(
                applications, Binder.getCallingPid(), Binder.getCallingUid());
        DesktopRootRegistry.Registration registration =
                roots.prepare(identity, incarnation, clientLifetime);
        Capability capability = new Capability(roots, registration);
        try {
            roots.linkAndPublish(registration);
            reply.writeNoException();
            reply.writeStrongBinder(capability);
            return true;
        } catch (RuntimeException | RemoteException failure) {
            roots.remove(registration);
            throw failure;
        }
    }

    private static final class Capability extends Binder {
        private final DesktopRootRegistry roots;
        private final DesktopRootRegistry.Registration registration;

        Capability(DesktopRootRegistry owner, DesktopRootRegistry.Registration value) {
            roots = owner;
            registration = value;
            attachInterface(null, DESCRIPTOR);
        }

        @Override
        protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
                throws RemoteException {
            if ((code != TRANSACTION_FACT && code != TRANSACTION_BIND_WINDOW
                    && code != TRANSACTION_ATTACH_FOCUS_DECISIONS) || reply == null) {
                return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
            }
            data.enforceInterface(DESCRIPTOR);
            if (code == TRANSACTION_ATTACH_FOCUS_DECISIONS) {
                int version = data.readInt();
                IBinder callback = data.readStrongBinder();
                data.enforceNoDataAvail();
                if (version != 1) throw new IllegalArgumentException("unsupported decision version");
                roots.attachFocusDecisions(registration, callback);
                reply.writeNoException();
                reply.writeInt(android.os.Process.myPid());
                reply.writeInt(android.os.Process.myUid());
                return true;
            }
            if (code == TRANSACTION_BIND_WINDOW) {
                int version = data.readInt();
                IBinder originalChannelToken = data.readStrongBinder();
                int displayId = data.readInt();
                data.enforceNoDataAvail();
                if (version != 1) throw new IllegalArgumentException("unsupported bind version");
                boolean retained;
                try {
                    retained = roots.bindWindow(registration, originalChannelToken, displayId);
                } catch (DesktopRootRegistry.BindingRejected rejected) {
                    // False is a definite pre-commit policy rejection. Any
                    // transport/driver failure still propagates as uncertain.
                    reply.writeNoException();
                    reply.writeBoolean(false);
                    return true;
                }
                reply.writeNoException();
                // This is retention acknowledgement only; it is not a focus grant.
                reply.writeBoolean(retained);
                return true;
            }
            int kind = data.readInt();
            long incarnation = data.readLong();
            long serial = data.readLong();
            boolean keySnapshot = data.readBoolean();
            data.enforceNoDataAvail();
            boolean retained = roots.recordFact(registration, kind, incarnation, serial,
                    keySnapshot);
            reply.writeNoException();
            // This is retention acknowledgement only; it is not a focus grant.
            reply.writeBoolean(retained);
            return true;
        }
    }
}
