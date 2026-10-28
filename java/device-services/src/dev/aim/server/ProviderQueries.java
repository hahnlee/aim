package dev.aim.server;

import android.content.ContentProvider;
import android.content.Context;
import android.os.UserHandle;
import android.os.UserManager;

/** ComputerEngine's clone-provider exception, android-16.0.0_r1. */
final class ProviderQueries extends IProviderQueries.Stub {
    private final Context context;

    ProviderQueries(Context context) { this.context = context; }

    @Override public boolean isCloneRedirected(String authority, int callingUid, int userId) {
        Bridge.enforceSystemUid();
        if (!ContentProvider.isAuthorityRedirectedForCloneProfile(authority)) return false;
        UserManager users = context.getSystemService(UserManager.class);
        if (users == null) throw new IllegalStateException("user clone owner unavailable");
        var info = users.getUserInfo(UserHandle.getUserId(callingUid));
        return info != null && info.isCloneProfile() && info.profileGroupId == userId;
    }
}
