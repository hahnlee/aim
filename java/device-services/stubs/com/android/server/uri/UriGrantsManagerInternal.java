// Compile-only original framework API.
package com.android.server.uri;

public interface UriGrantsManagerInternal {
    public abstract boolean checkAuthorityGrants(int uid, android.content.pm.ProviderInfo provider,
            int userId, boolean checkUser);
}
