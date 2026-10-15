package com.android.server.pm;

import android.content.ComponentName;
import android.content.IntentFilter;
import android.content.pm.ResolveInfo;
import android.os.Binder;
import android.os.UserHandle;
import dev.aim.server.PackageSnapshots;
import java.util.Objects;

/** Original forwarding record construction with native component metadata and original UM. */
public final class NativeForwardingResolveInfo {
    private NativeForwardingResolveInfo() {}
    public static ResolveInfo create(PackageSnapshots.ComputerSnapshot packages, String platformPackage,
            WatchedIntentFilter filter, int source, int target) {
        Objects.requireNonNull(platformPackage);
        boolean profile;
        long identity = Binder.clearCallingIdentity();
        try {
            android.content.pm.UserInfo selected = null;
            for (var user : NativeUserManagerBridge.getUserInfos()) if (user.id == target) { selected = user; break; }
            profile = Objects.requireNonNull(selected).isManagedProfile();
        } finally { Binder.restoreCallingIdentity(identity); }
        String name = profile ? "com.android.internal.app.ForwardIntentToManagedProfile"
                : "com.android.internal.app.ForwardIntentToParent";
        var activity = packages.getActivityInfoCrossProfile(new ComponentName(platformPackage, name), 0, source);
        var result = new ResolveInfo();
        if (!profile) { ((android.content.pm.PackageItemInfo) activity).showUserIcon = target; result.noResourceId = true; }
        result.activityInfo = activity; result.priority = 0; result.preferredOrder = 0; result.match = 0;
        result.isDefault = true; result.filter = new IntentFilter(filter.getIntentFilter());
        result.targetUserId = target; result.userHandle = UserHandle.of(source);
        return result;
    }
}
