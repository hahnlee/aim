// Compile-only pinned image API; checked against original image classes.
package com.android.internal.pm.pkg.component;
public interface ParsedProcess {
    String getName();
    android.util.ArrayMap<String, String> getAppClassNamesByPackage();
    java.util.Set<String> getDeniedPermissions();
    int getGwpAsanMode();
    int getMemtagMode();
    int getNativeHeapZeroInitialized();
    boolean isUseEmbeddedDex();
}
