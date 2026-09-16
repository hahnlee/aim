package dev.darwinart.runtime.system;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import android.util.Log;
import java.util.Collections;
import java.util.HashMap;
import java.util.Map;

/** System-owned startup service table; no application lifecycle or host transport. */
public final class ServiceDirectory extends Binder {
    private final Map<String, IBinder> services;

    public ServiceDirectory(Map<String, IBinder> services) {
        this.services = Collections.unmodifiableMap(new HashMap<>(services));
        attachInterface(null, "android.os.IServiceManager");
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        // android-16.0.0_r1 IServiceManager.aidl:
        // getService=1, getService2=2, checkService=3, checkService2=4.
        // ServiceManagerProxy redirects both public lookups to checkService2.
        // Publication/lazy-service contracts remain unsupported.
        if (code < FIRST_CALL_TRANSACTION || code > FIRST_CALL_TRANSACTION + 3) {
            return super.onTransact(code, data, reply, flags);
        }
        if (reply == null) return false;
        Log.i("DarwinSystem", "service lookup: enforce code=" + code);
        data.enforceInterface("android.os.IServiceManager");
        String name = data.readString();
        data.enforceNoDataAvail();
        Log.i("DarwinSystem", "service lookup: resolve name=" + name);
        IBinder service = services.get(name);
        boolean withMetadata = code == FIRST_CALL_TRANSACTION + 1
                || code == FIRST_CALL_TRANSACTION + 3;
        Log.i("DarwinSystem", "service lookup: metadata begin present=" + (service != null));
        Parcelable metadata = withMetadata ? metadata(service) : null;
        Log.i("DarwinSystem", "service lookup: metadata ready=" + (metadata != null));
        reply.writeNoException();
        if (withMetadata) {
            reply.writeTypedObject(metadata, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
        } else {
            reply.writeStrongBinder(service);
        }
        Log.i("DarwinSystem", "service lookup: reply complete name=" + name);
        return true;
    }

    private static Parcelable metadata(IBinder binder) {
        // Hidden generated AIDL DTOs are absent from the public compile SDK.
        // Construct the real boot-framework wire types, not framework state or
        // a service proxy. Every entry in this immutable table is non-lazy.
        try {
            Class<?> type = Class.forName("android.os.ServiceWithMetadata");
            Object value = type.getConstructor().newInstance();
            type.getField("service").set(value, binder);
            type.getField("isLazyService").setBoolean(value, false);
            return (Parcelable) Class.forName("android.os.Service")
                    .getMethod("serviceWithMetadata", type).invoke(null, value);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Framework service-manager AIDL mismatch", error);
        }
    }
}
