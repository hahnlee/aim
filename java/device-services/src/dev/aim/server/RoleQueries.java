package dev.aim.server;

import android.content.Context;
import com.android.role.controller.model.Role;
import com.android.role.controller.model.Roles;
import com.android.role.controller.util.RoleFlags;

/** RoleService.getActiveUserForRoleAsUser's definition checks, android-16.0.0_r1.
 * Copyright (C) The Android Open Source Project, Apache License 2.0. */
public final class RoleQueries extends IRoleQueries.Stub {
    // Role.EXCLUSIVITY_PROFILE_GROUP (inlined and removed from the image).
    private static final int EXCLUSIVITY_PROFILE_GROUP = 2;
    private final Context context;
    RoleQueries(Context context) { this.context = context; }

    @Override public void enforceProfileGroupExclusiveRole(String roleName) {
        Bridge.enforceSystemUid();
        if (!RoleFlags.isProfileGroupExclusivityAvailable())
            throw new IllegalStateException("getActiveUserForRoleAsUser not available");
        if (roleName == null || roleName.isEmpty())
            throw new IllegalArgumentException("roleName cannot be null or empty");
        Role role = Roles.get(context).get(roleName);
        if (role == null || role.getExclusivity() != EXCLUSIVITY_PROFILE_GROUP)
            throw new IllegalArgumentException(roleName + " is not a profile-group exclusive role");
    }
}
