package android.content.pm;

import android.os.Bundle;

public class ApplicationInfo extends PackageItemInfo {
    public static final int FLAG_SYSTEM = 1 << 0;
    public static final int FLAG_HAS_CODE = 1 << 1;
    public static final int FLAG_DEBUGGABLE = 1 << 2;
    public static final int FLAG_HARDWARE_ACCELERATED = 1 << 3;
    public static final int FLAG_SUPPORTS_RTL = 1 << 4;
    public String packageName;
    public String processName;
    public String sourceDir;
    public String publicSourceDir;
    public String[] splitNames;
    public String[] splitSourceDirs;
    public String[] splitPublicSourceDirs;
    public String dataDir;
    public String credentialProtectedDataDir;
    public String deviceProtectedDataDir;
    public String nativeLibraryDir;
    public String className;
    public Bundle metaData;
    public boolean enabled;
    public int flags;
    public int targetSdkVersion;
    public int uid;
}
