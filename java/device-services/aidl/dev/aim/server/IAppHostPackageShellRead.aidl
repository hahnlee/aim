package dev.aim.server;
/** Cold original shell presentation over scoped native read capabilities. */
interface IAppHostPackageShellRead {
    String[] dumpResolveInfo(in byte[] record, String prefix);
    String[] listPermissionGroups(IBinder permissions);
    String[] listPermissions(boolean groups, boolean labels, boolean summary,
            int minimum, int maximum, String group, IBinder packages, IBinder permissions);
}
