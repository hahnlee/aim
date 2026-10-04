// Compile-only pinned image API, checked by the device-services build.
package com.android.server.pm.verify.domain;
public class DomainVerificationService extends com.android.server.SystemService implements DomainVerificationManagerInternal {
    public DomainVerificationService(android.content.Context context,
            com.android.server.SystemConfig config, com.android.server.compat.PlatformCompat compat) {
        super(context); throw new RuntimeException("stub");
    }
    @Override public void onStart() { throw new RuntimeException("stub"); }
    public java.util.UUID generateNewId() { throw new RuntimeException("stub"); }
}
