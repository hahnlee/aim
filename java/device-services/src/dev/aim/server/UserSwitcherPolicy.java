package dev.aim.server;

import android.content.Context;
import android.content.pm.UserInfo;
import android.content.res.Resources;
import android.os.UserManager;
import android.provider.Settings;

/** UserManagerService.isUserSwitcherEnabled, android-16.0.0_r1.
 * Native user state and authenticated permission decisions replace mUsers/the caller.
 * Copyright (C) The Android Open Source Project, Apache License 2.0. */
final class UserSwitcherPolicy extends IUserSwitcherPolicy.Stub {
    private final Context context;
    UserSwitcherPolicy(Context context) { this.context = context; }

    @Override public boolean isEnabled(boolean showEvenIfNotActionable,
            boolean switchRestricted, boolean addRestricted, boolean canReadRestrictions,
            boolean canQueryUsers, UserInfo[] users) {
        Bridge.enforceSystemUid();
        boolean multiUserSettingOn = Settings.Global.getInt(context.getContentResolver(),
                "user_switcher_enabled", Resources.getSystem().getBoolean(
                        com.android.internal.R.bool.config_showUserSwitcherByDefault) ? 1 : 0) != 0;
        if (!UserManager.supportsMultipleUsers()) return false;
        if (!canReadRestrictions) {
            throw new SecurityException("You need INTERACT_ACROSS_USERS or MANAGE_USERS permission "
                    + "to: check hasUserRestriction");
        }
        if (switchRestricted || UserManager.isDeviceInDemoMode(context) || !multiUserSettingOn) {
            return false;
        }
        return showEvenIfNotActionable || !addRestricted
                || areThereMultipleSwitchableUsers(canQueryUsers, users);
    }

    private boolean areThereMultipleSwitchableUsers(boolean canQueryUsers, UserInfo[] users) {
        if (!canQueryUsers) {
            throw new SecurityException(
                    "You either need MANAGE_USERS or CREATE_USERS permission to: query users");
        }
        boolean isAnyAliveUser = false;
        for (UserInfo userInfo : users) {
            if (userInfo.supportsSwitchToByUser()) {
                if (isAnyAliveUser) return true;
                isAnyAliveUser = true;
            }
        }
        return false;
    }
}
