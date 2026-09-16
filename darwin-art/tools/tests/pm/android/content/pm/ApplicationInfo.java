package android.content.pm;

// Test-only DTO seam. Never included in runtime compilation or DEX packaging.
// This verifies record mapping, not Android ApplicationInfo/LoadedApk behavior.
public final class ApplicationInfo extends PackageItemInfo {
    public static final int FLAG_SYSTEM = 1;
    public static final int FLAG_DEBUGGABLE = 2;
    public static final int FLAG_HAS_CODE = 4;
    public static final int FLAG_HARDWARE_ACCELERATED = 1 << 27;
    public static final int FLAG_SUPPORTS_RTL = 1 << 22;
    public String packageName, processName, sourceDir, publicSourceDir, dataDir,
            deviceProtectedDataDir, className, nativeLibraryDir;
    private String credentialProtectedDataDir;
    public String[] splitSourceDirs, splitPublicSourceDirs;
    public boolean enabled;
    public int flags, targetSdkVersion, uid;
}
