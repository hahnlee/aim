package com.android.internal.pm.parsing;

import android.content.pm.PackageInfo;
import com.android.server.pm.pkg.AndroidPackage;

/**
 * Compile-only Android 16 signature stub; runtime resolution uses framework.jar.
 * Only {@code generate} is public; the per-component generators are private.
 */
public class PackageInfoCommonUtils {
    public static PackageInfo generate(AndroidPackage pkg, long flags, int userId) {
        throw new RuntimeException("stub");
    }
}
