package dev.aim.server;
interface IPackageBootConfigurationLeaf {
    int getDensity();
    boolean isFactoryTest();
    boolean isDependencyInstallerEnabled();
    boolean fixSystemAppsFirstInstallTime();
    byte[] creationInode(String guestPath);
    byte[] captureResources();
    byte[] getLiveProperties(in String[] keys);
    String getParserCacheDirectory();
    int[] getUserIds();
}
