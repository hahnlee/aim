package dev.aim.server;

/** Original owners needed before native PackageManager boot scanning. */
interface IPackageBootstrapBridge {
    boolean areNativeLibraryDependenciesEnforced(String packageName, int targetSdk);
    boolean isTestBaseOnBootclasspath();
    int[] getPermissionGidsForUid(int uid);
    /** Non-shared SELinuxMMAC compatibility decision for original parsed code. */
    int getSeInfoTargetSdkVersion(in byte[] packageCache);
    /** Projection of the original permission owner over resolved users, including pre-created users. */
    byte[] getLegacyPermissionState(int appId, in int[] userIds);
    /** UUID from the original domain owner, most-significant word first. */
    byte[] generateNewDomainId();
    /** Original resolved scan users, including pre-created users and ADB restrictions. */
    byte[] getPackageScanUsers();
    /** Original all-package and active-mount APEX inventories, preserving owner order. */
    byte[] getApexBootInventory();
}
