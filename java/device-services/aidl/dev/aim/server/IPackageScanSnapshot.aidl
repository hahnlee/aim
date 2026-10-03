package dev.aim.server;

/** One captured native scan version, retained until close. System UID only. */
interface IPackageScanSnapshot {
    long getVersion();
    String[] getPackageNames(boolean disabled);
    int getCodeLength(String packageName, boolean disabled);
    byte[] getCodeChunk(String packageName, boolean disabled, int offset, int length);
    void close();
    /** Captured active-package usage; null for a name outside this capture. */
    byte[] getUsage(String packageName);
}
