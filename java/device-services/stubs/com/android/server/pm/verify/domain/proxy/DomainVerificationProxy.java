// Compile-only pinned image API, checked by the device-services build.
package com.android.server.pm.verify.domain.proxy;
public interface DomainVerificationProxy {
    static <T extends DomainVerificationProxyV1.Connection & DomainVerificationProxyV2.Connection>
    DomainVerificationProxy makeProxy(android.content.ComponentName legacy,android.content.ComponentName modern,
            android.content.Context context,com.android.server.pm.verify.domain.DomainVerificationManagerInternal manager,
            com.android.server.pm.verify.domain.DomainVerificationCollector collector,T connection){throw new RuntimeException("stub");}
    interface BaseConnection {
        void schedule(int code,Object object);
        long getPowerSaveTempWhitelistAppDuration();
        com.android.server.DeviceIdleInternal getDeviceIdleInternal();
        boolean isCallerPackage(int uid,String name);
    }
    android.content.ComponentName getComponentName();
    void sendBroadcastForPackages(java.util.Set<String> names);
    boolean runMessage(int code, Object object);
    boolean isCallerVerifier(int uid);
}
