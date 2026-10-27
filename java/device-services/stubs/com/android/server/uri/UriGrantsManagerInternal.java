// Compile-only original framework API.
package com.android.server.uri;

public interface UriGrantsManagerInternal {
    public abstract android.os.IBinder newUriPermissionOwner(String name);
    public abstract void revokeUriPermissionFromOwner(android.os.IBinder owner,
            android.net.Uri uri, int modeFlags, int sourceUserId);
    public abstract boolean checkAuthorityGrants(int uid, android.content.pm.ProviderInfo provider,
            int userId, boolean checkUser);
}
