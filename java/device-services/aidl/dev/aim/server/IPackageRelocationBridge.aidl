package dev.aim.server;
import dev.aim.server.PackageRelocationVolume;
/** Storage primitives used by the native move transaction. */
interface IPackageRelocationBridge {
    boolean allowThirdPartyInternal();
    PackageRelocationVolume resolveVolume(String uuid);
    boolean isFileEncrypted();
    boolean isUserUnlocked(int user);
    int[] getUsers();
    String applicationLabel(String packageName, int user);
    long[] measurePackage(String volume, String packageName, int user, int appId, long ceInode, String codePath);
    long usableBytes(String path);
    long bytesUntilLow(String path);
    void prepareUsers(String fromVolume, String toVolume, in int[] users);
    void moveCompleteApp(String fromVolume, String toVolume, String packageName, int appId, String seinfo, int targetSdk, String fromCodePath);
}
