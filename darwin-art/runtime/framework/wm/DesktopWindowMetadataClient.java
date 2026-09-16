package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import android.util.Log;
import dev.darwinart.runtime.os.SystemServices;

/** App-process receiver that terminates desktop metadata at its display provider. */
public final class DesktopWindowMetadataClient {
    private static final Object LOCK = new Object();
    private static IBinder receiver;
    private static long lastGeneration;

    private DesktopWindowMetadataClient() {}

    private static native boolean nativeSetTitle(String title);

    public static void register() throws RemoteException {
        synchronized (LOCK) {
            if (receiver != null) return;
            IBinder local = new Binder() {
                {
                    attachInterface(null, DesktopWindowMetadataRegistry.RECEIVER_DESCRIPTOR);
                }

                @Override
                protected boolean onTransact(int code, Parcel data, Parcel reply, int flags) {
                    if (code != DesktopWindowMetadataRegistry.TRANSACTION_UPDATE) return false;
                    data.enforceInterface(DesktopWindowMetadataRegistry.RECEIVER_DESCRIPTOR);
                    data.readStrongBinder(); // WMS IWindow identity; one desktop target per process.
                    long generation = data.readLong();
                    String title = data.readString();
                    data.enforceNoDataAvail();
                    synchronized (LOCK) {
                        if (generation <= lastGeneration) return true;
                        lastGeneration = generation;
                    }
                    if (title != null && !title.isEmpty() && !nativeSetTitle(title)) {
                        Log.w("DarwinWindowMetadata", "desktop surface unavailable for title update");
                    }
                    return true;
                }
            };
            IBinder service = SystemServices.getService("darwin.window_metadata");
            if (service == null) throw new RemoteException("desktop metadata service unavailable");
            Parcel data = Parcel.obtain();
            Parcel reply = Parcel.obtain();
            try {
                data.writeInterfaceToken(DesktopWindowMetadataEndpoint.DESCRIPTOR);
                data.writeStrongBinder(local);
                if (!service.transact(
                        DesktopWindowMetadataEndpoint.TRANSACTION_REGISTER, data, reply, 0)) {
                    throw new RemoteException("desktop metadata registration rejected");
                }
                reply.readException();
                receiver = local;
            } finally {
                reply.recycle();
                data.recycle();
            }
        }
    }
}
