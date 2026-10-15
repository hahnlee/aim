package dev.aim.server;
/** Native usage/settings durability and actual boot dexopt timing owner. */
interface IPackageDexoptCompletion {
    long getBootDexoptStartTimeNanos();
    void noteBootDexoptStartTimeNanos(long startTimeNanos);
    int getOptimizablePackageCount();
    void persistPackageUsage();
}
