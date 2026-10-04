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
    /** Captured active seInfo base/override fields; unfinished phase is an error. */
    byte[] getSeInfo(String packageName);
    /** Saved package and shared-UID signing owners; collected code remains separate. */
    byte[] getSigningState(String packageName, boolean disabled);
    /** Persisted user state from this capture, paged to include large extras. */
    int getUserStateLength(String packageName, boolean disabled, int userId);
    byte[] getUserStateChunk(String packageName, boolean disabled, int userId, int offset, int length);
    /** Captured PackageSetting scalar owners, including nullable runtime old paths. */
    int getSettingLength(String packageName, boolean disabled);
    byte[] getSettingChunk(String packageName, boolean disabled, int offset, int length);
    /** Explicit sparse PackageSetting user entries; null only for an unknown setting. */
    int[] getUserStateIds(String packageName, boolean disabled);
    /** Finalized active dependency owner; missing/unresolved inputs are errors. */
    int getLibraryStateLength(String packageName);
    byte[] getLibraryStateChunk(String packageName, int offset, int length);
    /** Runtime setting flags and APEX ownership, never inferred from saved flags. */
    byte[] getTransientState(String packageName, boolean disabled);
}
