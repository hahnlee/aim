package dev.darwinart.runtime.pm;

import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageInfo;
import android.content.pm.ParceledListSlice;
import android.content.pm.ResolveInfo;
import android.content.ComponentName;
import android.content.pm.ServiceInfo;
import android.content.pm.ProviderInfo;
import android.os.Binder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import com.android.server.pm.dex.DexUsageStore;
import java.lang.reflect.Field;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** System PM Binder dispatch. Unsupported methods remain unsupported, never default-success. */
public final class PackageManagerEndpoint extends Binder {
    private final int notifyDexLoadCode = transaction("notifyDexLoad");
    private final int getApplicationInfoCode = transaction("getApplicationInfo");
    private final int getPackageInfoCode = transaction("getPackageInfo");
    private final int getServiceInfoCode = transaction("getServiceInfo");
    private final int getProviderInfoCode = transaction("getProviderInfo");
    private final int getTargetSdkVersionCode = transaction("getTargetSdkVersion");
    private final int queryIntentActivitiesCode = transaction("queryIntentActivities");
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

    /**
     * Intent resolution within one installed package (an explicit component or
     * package). Resolution across every installed package needs the package
     * index PackageManagerService keeps and is not answered here (#30, #24).
     */
    private List<ResolveInfo> queryPackageActivities(Intent intent, String resolvedType,
            long flags) {
        if (intent == null || intent.getSelector() != null) return null;
        ComponentName component = intent.getComponent();
        String packageName = component != null ? component.getPackageName() : intent.getPackage();
        if (packageName == null) return null;
        String record = packages.resolveInstalledPackage(packageName);
        if (record == null) return new java.util.ArrayList<>();
        if (component == null) {
            return InstalledPackageInfos.queryIntentActivities(packageName, record, intent,
                    resolvedType, flags);
        }
        java.util.ArrayList<ResolveInfo> explicit = new java.util.ArrayList<>();
        android.content.pm.ActivityInfo activity = InstalledPackageInfos.activity(packageName,
                record, component.getClassName(), flags);
        if (activity != null) {
            ResolveInfo resolved = new ResolveInfo();
            resolved.activityInfo = activity;
            explicit.add(resolved);
        }
        return explicit;
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags) throws RemoteException {
        if (code == getApplicationInfoCode) {
            data.enforceInterface("android.content.pm.IPackageManager");
            String packageName = data.readString();
            long queryFlags = data.readLong();
            int userId = data.readInt();
            data.enforceNoDataAvail();
            ApplicationInfo result = userId == 0 && packageName != null
                    ? InstalledPackageInfos.applicationInfo(packageName,
                            packages.resolveInstalledPackage(packageName), queryFlags)
                    : null;
            reply.writeNoException();
            reply.writeTypedObject(result, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code == getPackageInfoCode) {
            data.enforceInterface("android.content.pm.IPackageManager");
            String packageName = data.readString();
            long queryFlags = data.readLong();
            int userId = data.readInt();
            data.enforceNoDataAvail();
            PackageInfo result = userId == 0 && packageName != null
                    ? InstalledPackageInfos.packageInfo(packageName,
                            packages.resolveInstalledPackage(packageName), queryFlags)
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
                    : InstalledPackageInfos.applicationInfo(packageName,
                            packages.resolveInstalledPackage(packageName),
                            InstalledPackageInfos.STOCK_PM_FLAGS);
            reply.writeNoException();
            reply.writeInt(result == null ? -1 : result.targetSdkVersion);
            return true;
        }
        if (code == getServiceInfoCode) {
            data.enforceInterface("android.content.pm.IPackageManager");
            ComponentName component = data.readTypedObject(ComponentName.CREATOR);
            long queryFlags = data.readLong();
            int userId = data.readInt();
            data.enforceNoDataAvail();
            ServiceInfo result = null;
            if (userId == 0 && component != null) {
                String packageName = component.getPackageName();
                result = InstalledPackageInfos.service(packageName,
                        packages.resolveInstalledPackage(packageName), component.getClassName(),
                        queryFlags);
            }
            reply.writeNoException();
            reply.writeTypedObject(result, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code == queryIntentActivitiesCode) {
            data.enforceInterface("android.content.pm.IPackageManager");
            Intent intent = data.readTypedObject(Intent.CREATOR);
            String resolvedType = data.readString();
            long queryFlags = data.readLong();
            int userId = data.readInt();
            data.enforceNoDataAvail();
            List<ResolveInfo> result = userId == 0
                    ? queryPackageActivities(intent, resolvedType, queryFlags)
                    : null;
            if (result == null) return super.onTransact(code, data, reply, flags);
            reply.writeNoException();
            reply.writeTypedObject(new ParceledListSlice<>(result),
                    Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code == getProviderInfoCode) {
            data.enforceInterface("android.content.pm.IPackageManager");
            ComponentName component = data.readTypedObject(ComponentName.CREATOR);
            long queryFlags = data.readLong();
            int userId = data.readInt();
            data.enforceNoDataAvail();
            ProviderInfo result = null;
            if (userId == 0 && component != null) {
                String packageName = component.getPackageName();
                result = InstalledPackageInfos.provider(packageName,
                        packages.resolveInstalledPackage(packageName),
                        component.getClassName(), queryFlags);
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
