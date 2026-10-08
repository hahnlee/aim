package dev.aim.server;
/** Independent original syntax, Parcelable dump and permission/resource owners. */
interface IPackageShellReadLeaf {
    byte[] parseIntent(in String[] arguments);
    String[] dumpResolveInfo(in byte[] record, String prefix);
    String[] listPermissions(boolean groups, boolean labels, boolean summary, int minimum, int maximum, String group, int callingUid, int callingPid);
    String[] listInstrumentation(boolean showSourceDir, String targetPackage, int callingUid, int callingPid);
}
