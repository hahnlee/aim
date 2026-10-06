// Compile-only pinned image API, checked by the device-services build.
package com.android.server.pm.verify.domain;
public class DomainVerificationEnforcer {
    public DomainVerificationEnforcer(android.content.Context context) { throw new RuntimeException("stub"); }
    public interface Callback {
        boolean filterAppAccess(String name, int uid, int user);
        boolean doesUserExist(int user);
    }
}
