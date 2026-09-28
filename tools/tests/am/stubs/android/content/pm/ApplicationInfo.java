package android.content.pm;

import android.os.Bundle;

public class ApplicationInfo extends PackageItemInfo {
    public static final int FLAG_HAS_CODE = 1;
    public static final int FLAG_DEBUGGABLE = 2;
    public static final int FLAG_HARDWARE_ACCELERATED = 4;
    public static final int FLAG_SUPPORTS_RTL = 8;
    public static final int FLAG_SYSTEM = 16;
    public String packageName;
    public String processName;
    public String sourceDir;
    public String publicSourceDir;
    public String[] splitNames;
    public String[] splitSourceDirs;
    public String[] splitPublicSourceDirs;
    public String dataDir;
    public String deviceProtectedDataDir;
    public String nativeLibraryDir;
    public String credentialProtectedDataDir;
    public Bundle metaData;
    public boolean enabled;
    public int uid;
    public int targetSdkVersion;
    public int flags;
    public String className;
}
