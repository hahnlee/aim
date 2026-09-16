package dev.darwinart.system;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import dev.darwinart.runtime.pm.PackageRecords;
import dev.darwinart.runtime.system.SystemServiceFactory;

/** Process entry for the Darwin-hosted Android system service directory. */
public final class DarwinSystemServer {
    public static final String DESCRIPTOR = "dev.darwinart.system.IPackageRegistry";
    public static final int TRANSACTION_RESOLVE_PACKAGE = 1;

    private DarwinSystemServer() {}

    private static native String nativeResolvePackage(String packageName);

    public static Binder createServiceDirectory() {
        PackageRegistryBinder packages = new PackageRegistryBinder();
        return SystemServiceFactory.create(packages);
    }

    private static final class PackageRegistryBinder extends Binder
            implements PackageRecords.Source {
        @Override
        public String resolveInstalledPackage(String packageName) {
            return nativeResolvePackage(packageName);
        }

        PackageRegistryBinder() {
            attachInterface(null, DESCRIPTOR);
        }

        @Override
        protected boolean onTransact(int code, Parcel data, Parcel reply, int flags) {
            if (code != TRANSACTION_RESOLVE_PACKAGE || reply == null) return false;
            data.enforceInterface(DESCRIPTOR);
            String packageName = data.readString();
            data.enforceNoDataAvail();
            String record = nativeResolvePackage(packageName);
            reply.writeNoException();
            reply.writeString(record);
            return true;
        }
    }
}
