// Compile-only pinned image API, checked by the device-services build.
package com.android.server.pm.verify.domain.proxy;
public class DomainVerificationProxyV1 {
    public DomainVerificationProxyV1(android.content.Context context,com.android.server.pm.verify.domain.DomainVerificationManagerInternal manager,com.android.server.pm.verify.domain.DomainVerificationCollector collector,Connection connection,android.content.ComponentName component){throw new RuntimeException("stub");}
    public interface Connection extends DomainVerificationProxy.BaseConnection {
        com.android.server.pm.pkg.AndroidPackage getPackage(String name);
    }
}
