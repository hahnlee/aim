package dev.aim.server;

/** Original owners needed before native PackageManager boot scanning. */
interface IPackageBootstrapBridge {
    boolean areNativeLibraryDependenciesEnforced(String packageName, int targetSdk);
    boolean isTestBaseOnBootclasspath();
    int[] getPermissionGidsForUid(int uid);
    /** Non-shared SELinuxMMAC compatibility decision for original parsed code. */
    int getSeInfoTargetSdkVersion(in byte[] packageCache);
}
