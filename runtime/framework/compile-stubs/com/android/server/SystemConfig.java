package com.android.server;

import android.content.pm.FeatureInfo;
import android.util.ArrayMap;
import android.util.ArraySet;
import java.util.Set;

/** Compile-only Android 16 signature stub; runtime resolution uses framework.jar/services.jar. */
public class SystemConfig {
    public static SystemConfig getInstance() { throw new RuntimeException("stub"); }
    public ArrayMap<String, FeatureInfo> getAvailableFeatures() {
        throw new RuntimeException("stub");
    }
    public ArraySet<String> getHiddenApiWhitelistedApps() { throw new RuntimeException("stub"); }
    public Set<String> getInstallConstraintsAllowlist() { throw new RuntimeException("stub"); }
}
