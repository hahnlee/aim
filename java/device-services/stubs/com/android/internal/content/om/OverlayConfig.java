// Compile-only pinned image API.
package com.android.internal.content.om;

public class OverlayConfig {
    public OverlayConfig(java.io.File rootDirectory, java.util.function.Supplier<?> scannerFactory,
            PackageProvider packageProvider) { throw new RuntimeException("stub"); }
    public static OverlayConfig initializeSystemInstance(PackageProvider provider) {
        throw new RuntimeException("stub");
    }
    public interface PackageProvider {
        void forEachPackage(com.android.internal.util.function.TriConsumer<Package, Boolean, java.io.File> consumer);
        interface Package {
            String getBaseApkPath();
            int getOverlayPriority();
            String getOverlayTarget();
            String getPackageName();
            int getTargetSdkVersion();
            boolean isOverlayIsStatic();
        }
    }
}
