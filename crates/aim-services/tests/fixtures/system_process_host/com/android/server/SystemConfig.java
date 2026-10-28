package com.android.server;
/** SystemConfig boundary double: an image without camera privacy allowlist entries. */
public class SystemConfig {
    private static final SystemConfig INSTANCE = new SystemConfig();
    public static SystemConfig getInstance() { return INSTANCE; }
    public android.util.ArraySet<String> getCameraPrivacyAllowlist() { return new android.util.ArraySet<>(); }
}
