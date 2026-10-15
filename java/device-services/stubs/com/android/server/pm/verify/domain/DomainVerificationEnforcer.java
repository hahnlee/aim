// Compile-only pinned image API, checked by the device-services build.
package com.android.server.pm.verify.domain;
public class DomainVerificationEnforcer {
    public DomainVerificationEnforcer(android.content.Context context) { throw new RuntimeException("stub"); }
    public void setCallback(Callback callback) { throw new RuntimeException("stub"); }
    public void assertInternal(int uid) { throw new RuntimeException("stub"); }
    public void assertApprovedQuerent(int uid, com.android.server.pm.verify.domain.proxy.DomainVerificationProxy proxy) { throw new RuntimeException("stub"); }
    public void assertApprovedVerifier(int uid, com.android.server.pm.verify.domain.proxy.DomainVerificationProxy proxy) { throw new RuntimeException("stub"); }
    public boolean assertApprovedUserStateQuerent(int uid, int caller, String name, int target) { throw new RuntimeException("stub"); }
    public boolean assertApprovedUserSelector(int uid, int caller, String name, int target) { throw new RuntimeException("stub"); }
    public void assertOwnerQuerent(int uid, int caller, int target) { throw new RuntimeException("stub"); }
    public boolean callerIsLegacyUserSelector(int uid, int caller, String name, int target) { throw new RuntimeException("stub"); }
    public boolean callerIsLegacyUserQuerent(int uid, int caller, String name, int target) { throw new RuntimeException("stub"); }
    public interface Callback {
        boolean filterAppAccess(String name, int uid, int user);
        boolean doesUserExist(int user);
    }
}
