package dev.aim.server;
import android.os.ParcelFileDescriptor;
/** Cold original shell presentation over scoped native read capabilities. */
interface IAppHostPackageShellRead {
    String[] dumpResolveInfo(in byte[] record, String prefix);
    String[] listPermissionGroups(IBinder permissions);
    String[] listPermissions(boolean groups, boolean labels, boolean summary,
            int minimum, int maximum, String group, IBinder packages, IBinder permissions);
    int runReadCommand(in String[] arguments, IBinder packages, IBinder activity, IBinder meminfo,
            IBinder procstats, IBinder usagestats, IBinder batterystats, in ParcelFileDescriptor input,
            in ParcelFileDescriptor output, in ParcelFileDescriptor error);
}
