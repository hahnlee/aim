package com.android.server.pm;

/**
 * Native PM's pinned android-16.0.0_r1 protocol values. AOSP, Apache-2.0.
 * IInstalld.aidl (frameworks/native cmds/installd/binder/android/os),
 * PackageManagerService.java and pm/dex/DexoptOptions.java (frameworks/base).
 * The original image strips these source constants; keep them in our owner.
 */
public final class NativePackageConstants {
    private NativePackageConstants() {}
    // services/core/java/com/android/server/EventLogTags.logtags.
    public static final int PM_CLEAR_APP_DATA_CALLER = 3132;
    // PackageManagerInternal.ExternalSourcesPolicy (same pinned AOSP tag).
    public static final int USER_TRUSTED = 0;
    public static final int USER_BLOCKED = 1;
    public static final int USER_DEFAULT = 2;
    public static final int FLAG_STORAGE_DE = 0x1;
    public static final int FLAG_STORAGE_CE = 0x2;
    public static final int FLAG_STORAGE_EXTERNAL = 0x4;
    public static final int FLAG_CLEAR_CACHE_ONLY = 0x10;
    public static final int FLAG_CLEAR_CODE_CACHE_ONLY = 0x20;
    public static final int FLAG_FREE_CACHE_V2 = 0x100;
    public static final int FLAG_FREE_CACHE_V2_DEFY_QUOTA = 0x200;
    public static final int FLAG_FREE_CACHE_DEFY_TARGET_FREE_BYTES = 0x800;
    public static final int REASON_INSTALL = 3;
    public static final int REASON_INSTALL_FAST = 4;
    public static final int REASON_INSTALL_BULK = 5;
    public static final int REASON_INSTALL_BULK_SECONDARY = 6;
    public static final int REASON_INSTALL_BULK_DOWNGRADED = 7;
    public static final int REASON_INSTALL_BULK_SECONDARY_DOWNGRADED = 8;
    public static final int REASON_CMDLINE = 12;
    public static final int DEXOPT_CHECK_FOR_PROFILES_UPDATES = 1 << 0;
    public static final int DEXOPT_FORCE = 1 << 1;
    public static final int DEXOPT_BOOT_COMPLETE = 1 << 2;
    public static final int DEXOPT_ONLY_SECONDARY_DEX = 1 << 3;
    public static final int DEXOPT_INSTALL_WITH_DEX_METADATA_FILE = 1 << 10;
    public static final int DEXOPT_FOR_RESTORE = 1 << 11;
}
