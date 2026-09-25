package com.android.server.pm.pkg;

import com.android.internal.pm.pkg.component.ParsedActivity;
import com.android.internal.pm.pkg.component.ParsedProvider;
import com.android.internal.pm.pkg.component.ParsedService;
import java.util.List;

/** Compile-only Android 16 signature stub; runtime resolution uses framework.jar/services.jar. */
public interface AndroidPackage {
    String getPackageName();
    String getBaseApkPath();
    String[] getSplitCodePaths();
    List<ParsedActivity> getActivities();
    List<ParsedActivity> getReceivers();
    List<ParsedService> getServices();
    List<ParsedProvider> getProviders();
}
