package dev.darwinart.runtime.am;

import android.os.Process;
import android.os.UserHandle;

/**
 * UserController.handleIncomingUser for the one user this system runs (user
 * 0, always current): the user a caller's request is served as, with the
 * cross-user permission rules of ActivityManagerService.
 */
final class IncomingUsers {
    /** The running users' current user. */
    static final int CURRENT_USER = UserHandle.USER_SYSTEM;

    interface Permissions {
        boolean granted(int uid, String permission);
    }

    private final Permissions permissions;

    IncomingUsers(Permissions permissions) {
        this.permissions = permissions;
    }

    /**
     * @param requireFull ALLOW_FULL_ONLY when true, otherwise
     *        ALLOW_NON_FULL (INTERACT_ACROSS_USERS is enough)
     */
    int handle(int callingUid, int userId, boolean allowAll, boolean requireFull,
            String name) {
        final int callingUserId = UserHandle.getUserId(callingUid);
        if (callingUserId == userId) return userId;
        // unsafeConvertIncomingUser.
        int targetUserId = userId == UserHandle.USER_CURRENT
                || userId == UserHandle.USER_CURRENT_OR_SELF ? CURRENT_USER : userId;
        final int appId = UserHandle.getAppId(callingUid);
        if (callingUid != 0 && appId != Process.SYSTEM_UID
                && targetUserId != callingUserId) {
            boolean allow = permissions.granted(callingUid,
                    android.Manifest.permission.INTERACT_ACROSS_USERS_FULL)
                    || (!requireFull && permissions.granted(callingUid,
                            android.Manifest.permission.INTERACT_ACROSS_USERS));
            if (!allow) {
                if (userId != UserHandle.USER_CURRENT_OR_SELF) {
                    throw new SecurityException("Permission Denial: " + name
                            + " from uid " + callingUid + " asks to run as user " + userId
                            + " but is calling from uid " + callingUid + "; this requires "
                            + (requireFull
                                    ? android.Manifest.permission.INTERACT_ACROSS_USERS_FULL
                                    : android.Manifest.permission.INTERACT_ACROSS_USERS));
                }
                targetUserId = callingUserId;
            }
        }
        if (!allowAll && targetUserId < 0) {
            throw new IllegalArgumentException(
                    "Call does not support special user #" + targetUserId);
        }
        return targetUserId;
    }
}
