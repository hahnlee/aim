package dev.aim.server;

/** Retained original process/lifecycle owners for the native package writer. */
interface IPackageMutationBridge {
    boolean requireInstallerPermission(int callingUid);
    void killPackage(String packageName, int appId, int userId, String reason, int exitReason);
    boolean isActiveDeviceAdmin(String packageName, int userId);
    boolean isStateProtected(String packageName, int userId);
    boolean isDataProtected(String packageName, int userId);
    String getOwnerPackage(int userId);
    void setDeviceAndProfileOwnerPackages(int deviceOwnerUserId, String deviceOwnerPackage,
            in int[] profileUserIds, in String[] profilePackages);
    void setOwnerProtectedPackages(int userId, in String[] packages);
    void packageChanged(String packageName, int packageUid, boolean dontKillApp,
            in String[] components, String reason, int callingUid, boolean delayed);
    void componentLabelChanged(String packageName, int packageUid, String componentClass, int callingUid);
    void firstLaunch(String packageName, String installerPackage, int userId);
    void unhibernate(String packageName, int userId);
    void packageUnstopped(String packageName, int userId);
    void packageAdded(String packageName, int userId, boolean archived, int dataLoaderType, String predictionPackage);
    void packageHidden(String packageName, int userId, boolean hidden);
    boolean isSuspensionAllowed(int userId, int callingUid);
    boolean[] canSuspendPackages(in String[] packages, int userId, int callingUid);
    void packagesSuspended(in String[] packages, in int[] uids, int userId,
            boolean suspended, boolean quarantined, boolean changedOnly);
    void removedSuspensions(in String[] packages, in int[] uids, int userId);
    void distractionChanged(in String[] packages, in int[] uids, int userId, int flags);
    void registerPackageMonitor(in IBinder callback, int userId, int callingUid);
    void unregisterPackageMonitor(in IBinder callback);
    void userRemoved(int userId);
    void postInstallPackage(String name, int appId, boolean replacing, boolean dontKill,
        String installer, String oldInstaller, int dataLoaderType, boolean system, boolean virtualPreload, boolean staticLibrary,
        in int[] firstUsers, in int[] firstInstantUsers, in int[] updateUsers, in int[] updateInstantUsers,
        in int[] removedUsers, in int[] removedInstantUsers, in int[] priorVisibility);
    int[] capturePostInstallVisibility(String name, int appId, String codePath, long version, in int[] users);

}
