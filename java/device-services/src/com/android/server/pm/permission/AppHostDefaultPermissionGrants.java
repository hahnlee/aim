// Copyright (C) The Android Open Source Project, Apache License 2.0.
package com.android.server.pm.permission;

import android.content.Context;
import dev.aim.server.IPackageDefaultGrantQueries;
import dev.aim.server.PackageLocalOwners;

/** PMS.systemReady's synchronous default grants, over the native permission Binder owner. */
public final class AppHostDefaultPermissionGrants {
    private final DefaultPermissionGrantPolicy policy;

    public AppHostDefaultPermissionGrants(Context context, IPackageDefaultGrantQueries queries) {
        PackageLocalOwners.prepareDefaultGrants(queries);
        policy = new DefaultPermissionGrantPolicy(context);
    }

    public void grant(int userId) {
        policy.grantDefaultPermissions(userId);
    }
}
