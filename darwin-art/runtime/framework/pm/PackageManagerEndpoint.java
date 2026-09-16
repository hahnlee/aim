package dev.darwinart.runtime.pm;

import android.content.pm.ApplicationInfo;
import android.content.pm.PackageInfo;
import android.content.ComponentName;
import android.content.pm.ServiceInfo;
import android.os.Binder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import com.android.server.pm.dex.DexUsageStore;
import java.lang.reflect.Field;
import java.util.HashMap;
import java.util.Map;

/** System PM Binder dispatch. Unsupported methods remain unsupported, never default-success. */
public final class PackageManagerEndpoint extends Binder {
    private final int notifyDexLoadCode = transaction("notifyDexLoad");
    private final int getApplicationInfoCode = transaction("getApplicationInfo");
    private final int getPackageInfoCode = transaction("getPackageInfo");
    private final int getServiceInfoCode = transaction("getServiceInfo");
    private final int getTargetSdkVersionCode = transaction("getTargetSdkVersion");
    private final PackageRecords.Source packages;
    private final DexLoadReports reports;

    public PackageManagerEndpoint(PackageRecords.Source packages) {
        this(packages, null);
    }

    public PackageManagerEndpoint(PackageRecords.Source packages,
            DexLoadReports.CallerIdentity callerIdentity) {
        attachInterface(null, "android.content.pm.IPackageManager");
        this.packages = packages;
        reports = new DexLoadReports(packages, new DexUsageStore(), callerIdentity);
    }

    private static int transaction(String name) {
        try {
            Field field = Class.forName("android.content.pm.IPackageManager$Stub")
                    .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags) throws RemoteException {
        if (code == getApplicationInfoCode) {
            data.enforceInterface("android.content.pm.IPackageManager");
            String packageName = data.readString();
            data.readLong(); // query flags; the current registry has one installed-state view.
            int userId = data.readInt();
            data.enforceNoDataAvail();
            ApplicationInfo result = userId == 0 && packageName != null
                    ? InstalledApplicationInfo.fromRecord(
                            packageName, packages.resolveInstalledPackage(packageName))
                    : null;
            reply.writeNoException();
            reply.writeTypedObject(result, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code == getPackageInfoCode) {
            data.enforceInterface("android.content.pm.IPackageManager");
            String packageName = data.readString();
            data.readLong();
            int userId = data.readInt();
            data.enforceNoDataAvail();
            PackageInfo result = userId == 0 && packageName != null
                    ? InstalledPackageInfo.fromRecord(
                            packageName, packages.resolveInstalledPackage(packageName))
                    : null;
            reply.writeNoException();
            reply.writeTypedObject(result, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code == getTargetSdkVersionCode) {
            data.enforceInterface("android.content.pm.IPackageManager");
            String packageName = data.readString();
            data.enforceNoDataAvail();
            ApplicationInfo result = packageName == null ? null
                    : InstalledApplicationInfo.fromRecord(
                            packageName, packages.resolveInstalledPackage(packageName));
            reply.writeNoException();
            reply.writeInt(result == null ? -1 : result.targetSdkVersion);
            return true;
        }
        if (code == getServiceInfoCode) {
            data.enforceInterface("android.content.pm.IPackageManager");
            ComponentName component = data.readTypedObject(ComponentName.CREATOR);
            data.readLong(); // query flags; the installed record is the authoritative view.
            int userId = data.readInt();
            data.enforceNoDataAvail();
            ServiceInfo result = null;
            if (userId == 0 && component != null) {
                String packageName = component.getPackageName();
                result = InstalledServiceInfo.service(packageName,
                        packages.resolveInstalledPackage(packageName), component.getClassName());
            }
            reply.writeNoException();
            reply.writeTypedObject(result, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code != notifyDexLoadCode) return super.onTransact(code, data, reply, flags);
        data.enforceInterface("android.content.pm.IPackageManager");
        String packageName = data.readString();
        // AIDL typed Map<String,String>: count followed by String key/value pairs.
        int count = data.readInt();
        if (count > data.dataAvail() / 8) throw new IllegalArgumentException("Truncated DEX report");
        Map<String, String> contexts = count < 0 ? null : new HashMap<String, String>();
        for (int i = 0; i < count; i++) contexts.put(data.readString(), data.readString());
        String isa = data.readString();
        data.enforceNoDataAvail();
        reports.report(Binder.getCallingUid(), packageName, contexts, isa);
        // This is an original oneway method. No fabricated reply Parcel.
        return true;
    }
}
