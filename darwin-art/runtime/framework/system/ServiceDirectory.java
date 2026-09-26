package dev.darwinart.runtime.system;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import android.util.Log;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * System-owned service table: the startup services plus those AOSP system
 * services publish with ServiceManager.addService. No application lifecycle or
 * host transport.
 */
public final class ServiceDirectory extends Binder {
    // android-16.0.0_r1 IServiceManager.aidl.
    private static final int TRANSACTION_ADD_SERVICE = FIRST_CALL_TRANSACTION + 4;
    private static final int TRANSACTION_LIST_SERVICES = FIRST_CALL_TRANSACTION + 5;
    private final Map<String, IBinder> services;
    private final ServiceDirectoryAdmission admission = new ServiceDirectoryAdmission();

    public ServiceDirectory(Map<String, IBinder> services) {
        this.services = new ConcurrentHashMap<>(services);
        attachInterface(null, "android.os.IServiceManager");
    }

    /** Native system startup calls this only after genuine Context/policy initialization. */
    public void publishApplicationLookups() { admission.publish(); }

    /**
     * A service published in this process, as its local Binder: libbinder
     * resolves an in-process lookup to the local object, never a proxy.
     */
    IBinder localService(String name) {
        return name == null ? null : services.get(name);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        // android-16.0.0_r1 IServiceManager.aidl:
        // getService=1, getService2=2, checkService=3, checkService2=4.
        // ServiceManagerProxy redirects both public lookups to checkService2.
        // Lazy services, notifications and declared (VINTF) instances remain
        // unsupported.
        if (code == TRANSACTION_ADD_SERVICE) return addService(data, reply);
        if (code == TRANSACTION_LIST_SERVICES) return listServices(data, reply);
        if (code < FIRST_CALL_TRANSACTION || code > FIRST_CALL_TRANSACTION + 3) {
            return super.onTransact(code, data, reply, flags);
        }
        if (reply == null) return false;
        admission.enforceLookup();
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

    /**
     * servicemanager admits publication only from system identities; here that
     * is the system process itself, which hosts every AOSP system service.
     */
    private boolean addService(Parcel data, Parcel reply) {
        if (reply == null) return false;
        admission.enforcePublication();
        data.enforceInterface("android.os.IServiceManager");
        String name = data.readString();
        IBinder service = data.readStrongBinder();
        data.readBoolean(); // allowIsolated: isolated processes are not admitted yet
        data.readInt(); // dumpPriority
        data.enforceNoDataAvail();
        if (name == null || name.isEmpty() || name.length() > 127 || service == null) {
            throw new IllegalArgumentException("Invalid service registration: " + name);
        }
        services.put(name, service);
        Log.i("DarwinSystem", "service published: " + name);
        reply.writeNoException();
        return true;
    }

    private boolean listServices(Parcel data, Parcel reply) {
        if (reply == null) return false;
        admission.enforceLookup();
        data.enforceInterface("android.os.IServiceManager");
        data.readInt(); // dumpPriority: every service is listed at every priority
        data.enforceNoDataAvail();
        ArrayList<String> names = new ArrayList<>(services.keySet());
        Collections.sort(names);
        reply.writeNoException();
        reply.writeStringArray(names.toArray(new String[0]));
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
