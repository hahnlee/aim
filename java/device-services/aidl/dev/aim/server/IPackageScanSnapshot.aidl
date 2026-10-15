package dev.aim.server;

import dev.aim.server.IPackageComputer;

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
    /** Native policy over this capture's code and image SystemConfig. */
    int getHiddenApiEnforcementPolicy(String packageName, boolean disabled);
    /** Complete shared UID inventory in this capture. */
    String[] getSharedUserNames();
    int getSharedUserStateLength(String sharedUserName);
    byte[] getSharedUserStateChunk(String sharedUserName, int offset, int length);
    /** Explicit runtime owners for each active/factory setting, including unloaded code. */
    int getRuntimeStateLength(String packageName, boolean disabled);
    byte[] getRuntimeStateChunk(String packageName, boolean disabled, int offset, int length);
    /** Query lease independent of this scan lease, against exactly the same version. */
    IPackageComputer getComputer();
    /** Metadata identity is retained only for sealed usage or isolated-query-only deltas. */
    long getMetadataVersion();
    int getUsageRecordsLength();
    byte[] getUsageRecordsChunk(int offset, int length);
    /** Lease-owned opaque comparison capability, never a snapshot version. */
    long getMetadataComparisonId();
    /** Null requires a full capture (expired/foreign/global change). Otherwise exact changed user-record keys. */
    byte[] getChangedUsersForMetadataBase(long comparisonId);
}
