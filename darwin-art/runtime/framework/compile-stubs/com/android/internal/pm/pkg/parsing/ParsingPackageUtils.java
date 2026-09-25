package com.android.internal.pm.pkg.parsing;

import java.util.Set;

/** Compile-only Android 16 signature stub; runtime resolution uses framework.jar/services.jar. */
public class ParsingPackageUtils {
    /** Collect APK signing certificates while parsing. */
    public static final int PARSE_COLLECT_CERTIFICATES = 1 << 5;

    public interface Callback {
        boolean hasFeature(String feature);
        Set<String> getHiddenApiWhitelistedApps();
        Set<String> getInstallConstraintsAllowlist();
    }
}
