// Compile-only pinned image API, checked by the device-services build.
package com.android.server.pm.verify.domain;
public class DomainVerificationManagerStub extends android.content.pm.verify.domain.IDomainVerificationManager.Stub {
    public DomainVerificationManagerStub(DomainVerificationService service) { throw new RuntimeException("stub"); }
    public java.util.List<String> queryValidVerificationPackageNames() { throw new RuntimeException("stub"); }
    public android.content.pm.verify.domain.DomainVerificationInfo getDomainVerificationInfo(String name) { throw new RuntimeException("stub"); }
    public int setDomainVerificationStatus(String id, android.content.pm.verify.domain.DomainSet domains, int state) { throw new RuntimeException("stub"); }
}
