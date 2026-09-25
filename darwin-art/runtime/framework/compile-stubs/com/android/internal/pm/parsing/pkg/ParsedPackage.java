package com.android.internal.pm.parsing.pkg;

import com.android.server.pm.pkg.AndroidPackage;

/** Compile-only Android 16 signature stub; runtime resolution uses framework.jar/services.jar. */
public interface ParsedPackage extends AndroidPackage {
    AndroidPackageInternal hideAsFinal();
}
