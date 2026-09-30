// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.servicewatcher;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.os.Bundle;

public final class CurrentUserServiceSupplier extends BroadcastReceiver implements
        ServiceWatcher.ServiceSupplier<CurrentUserServiceSupplier.BoundServiceInfo> {
    public static class BoundServiceInfo extends ServiceWatcher.BoundServiceInfo {
        protected BoundServiceInfo(String action, android.content.pm.ResolveInfo resolveInfo) { super(null, 0, null); throw new RuntimeException("stub"); }
        public Bundle getMetadata() { throw new RuntimeException("stub"); }
    }

    private CurrentUserServiceSupplier(Context context, String action, String explicitPackage,
            String callerPermission, String servicePermission, boolean matchSystemAppsOnly) { throw new RuntimeException("stub"); }

    public static CurrentUserServiceSupplier createFromConfig(Context context, String action,
            int enableOverlayResId, int nonOverlayPackageResId) { throw new RuntimeException("stub"); }

    @Override
    public void onReceive(Context context, Intent intent) { throw new RuntimeException("stub"); }
}
