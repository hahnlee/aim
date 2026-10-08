package dev.aim.server;
import dev.aim.server.IPackageBootLifecycleEvents;
import dev.aim.server.IPackageDexoptCompletion;
interface IPackageBootLifecycleLeaf {
    void prepareReady();
    void registerStorageListeners(IPackageBootLifecycleEvents events);
    void dexOptimizerReady();
    void registerPackageObservers(IPackageBootLifecycleEvents events);
    void upgradeDexopt(String reason, IPackageDexoptCompletion completion);
    boolean isHibernationSuppressed(String packageName);
    void performFstrim();
    void scheduleMaintenance();
    int apexSessionState(int sessionId);
    void close();
    void releaseCompressedBlocks(in String[] compressedCodePaths);
}
