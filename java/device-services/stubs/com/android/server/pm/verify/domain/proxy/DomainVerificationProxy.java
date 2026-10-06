// Compile-only pinned image API, checked by the device-services build.
package com.android.server.pm.verify.domain.proxy;
public interface DomainVerificationProxy {
    void sendBroadcastForPackages(java.util.Set<String> names);
    boolean runMessage(int code, Object object);
    boolean isCallerVerifier(int uid);
}
