// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.wm;

public abstract class ActivityTaskManagerInternal {
    public abstract void registerActivityStartInterceptor(int id, ActivityInterceptorCallback callback);
    public abstract PackageConfigurationUpdater createPackageConfigurationUpdater(String packageName, int userId);

    public interface PackageConfigurationUpdater {
        PackageConfigurationUpdater setNightMode(int nightMode);
        boolean commit();
    }
}
