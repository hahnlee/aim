package dev.aim.server;
/** Live original SystemProperties getter; no init Rc/RefCell crosses threads. */
interface IPackageInitialContextLeaf {
    String getLiveProperty(String name);
    byte[] getLiveProperties(in String[] names);
    byte[] captureImageScanInputs();
    byte[] getPermissionGroup(String name,int flags);
    boolean isInstallDisabled(String name,int uid,int user);
    int getAutoRevoke(String name,int uid);
    byte[] getAdminFacts(String name);
    int getInstallLocation();
    String[] getMountedPrivateVolumes();
    boolean isIncrementalPath(String path);
    long zipEntryTime(int dosTime);
    byte[] nativeLibraryInode(String path);
    String getSdkSandboxServiceAction();
    String[] getBootClassPathApexModules();
}
