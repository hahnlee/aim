// Copyright (C) The Android Open Source Project, Apache License 2.0.
package com.android.server.pm.permission;

import android.content.Context;
import android.content.pm.PackageManagerInternal;
import com.android.server.LocalServices;
import dev.aim.server.IPackageDefaultGrantQueries;
import dev.aim.server.NativePackageManagerInternal;

/** PMS.systemReady's synchronous default grants, over the native permission Binder owner. */
public final class AppHostDefaultPermissionGrants {
    private final DefaultPermissionGrantPolicy policy;

    public AppHostDefaultPermissionGrants(Context context, IPackageDefaultGrantQueries queries) {
        if (LocalServices.getService(PackageManagerInternal.class) != null) {
            throw new IllegalStateException("app-host package local owner already installed");
        }
        LocalServices.addService(PackageManagerInternal.class,
                new NativePackageManagerInternal(queries));
        policy = new DefaultPermissionGrantPolicy(context);
    }

    public void grant(int userId) {
        policy.grantDefaultPermissions(userId);
    }
}
