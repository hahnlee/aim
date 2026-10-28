package dev.aim.server;

import android.content.pm.*;
import android.os.*;
import android.permission.IPermissionManager;
import java.util.List;

/** Original Parcelable/resource formatting; native owners retain caller checks. */
final class AppHostPackageShellRead extends IAppHostPackageShellRead.Stub {
    private final android.content.Context context;
    AppHostPackageShellRead(android.content.Context context) { this.context = context; }
    @Override public int runReadCommand(String[] args, IBinder packages, IBinder activity,
            IBinder meminfo, IBinder procstats, IBinder usagestats, IBinder batterystats, ParcelFileDescriptor input,
            ParcelFileDescriptor output, ParcelFileDescriptor error) {
        if (Binder.getCallingUid() != 0 && Binder.getCallingUid() != 2000)
            throw new SecurityException("Shell commands are only callable by ADB");
        return com.android.server.pm.NativePackageReadCommand.run(context,
                IPackageManager.Stub.asInterface(packages),
                new IBinder[]{packages, activity, meminfo, procstats, usagestats, batterystats},
                args, input, output, error);
    }
    private static void enforce() {
        if (Binder.getCallingUid() != 1000) throw new SecurityException("Native shell owner required");
    }
    @Override public String[] dumpResolveInfo(byte[] record, String prefix) {
        enforce();
        return NativePackageShellReadLeaf.dump(record, prefix);
    }
    @Override public String[] listPermissionGroups(IBinder permissions) throws RemoteException {
        enforce();
        return NativePackageShellReadLeaf.permissionGroups(
                IPermissionManager.Stub.asInterface(permissions).getAllPermissionGroups(0).getList());
    }
    @Override public String[] listPermissions(boolean groups, boolean labels, boolean summary,
            int minimum, int maximum, String group, IBinder packages, IBinder permissions)
            throws RemoteException {
        enforce();
        var pm = IPackageManager.Stub.asInterface(packages);
        var owner = IPermissionManager.Stub.asInterface(permissions);
        return NativePackageShellReadLeaf.permissions(groups, labels, summary, minimum, maximum,
                group, pm, new NativePackageShellReadLeaf.PermissionQueries() {
                    public List<PermissionGroupInfo> groups() throws RemoteException {
                        return owner.getAllPermissionGroups(0).getList();
                    }
                    public PermissionGroupInfo group(String name) throws RemoteException {
                        return owner.getPermissionGroupInfo(name, 0);
                    }
                    public List<PermissionInfo> permissions(String name) throws RemoteException {
                        var result = owner.queryPermissionsByGroup(name, 0);
                        return result == null ? null : result.getList();
                    }
                });
    }
}
