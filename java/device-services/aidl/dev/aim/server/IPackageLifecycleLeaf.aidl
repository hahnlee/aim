package dev.aim.server;

/** Original storage state, independent of UserManager unlock progress. */
interface IPackageLifecycleLeaf {
    boolean isCeStorageUnlocked(int userId);
}
