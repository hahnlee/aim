// Compile-only image API; checked by the device-services build node.
package com.android.server.pm.pkg;
public interface PackageUserStateInternal extends PackageUserState, android.content.pm.pkg.FrameworkPackageUserState {
    com.android.server.utils.WatchedArrayMap<android.content.pm.UserPackage, SuspendParams> getSuspendParams();
    com.android.server.utils.WatchedArraySet<String> getEnabledComponentsNoCopy();
    com.android.server.utils.WatchedArraySet<String> getDisabledComponentsNoCopy();
    android.util.Pair<String, Integer> getOverrideLabelIconForComponent(android.content.ComponentName componentName);
}
