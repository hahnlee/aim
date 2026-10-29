package com.android.server.pm.verify.domain.proxy;
/** Legacy transport double; the factory, combined proxy and V2 run unchanged. */
public final class DomainVerificationProxyV1 implements DomainVerificationProxy {
    public static int sends;
    private final android.content.ComponentName component;
    public interface Connection extends BaseConnection {com.android.server.pm.pkg.AndroidPackage getPackage(String name);}
    public DomainVerificationProxyV1(android.content.Context context,com.android.server.pm.verify.domain.DomainVerificationManagerInternal manager,com.android.server.pm.verify.domain.DomainVerificationCollector collector,Connection connection,android.content.ComponentName component){this.component=component;}
    public void sendBroadcastForPackages(java.util.Set<String> names){sends++;}
    public boolean runMessage(int code,Object value){return false;}
    public boolean isCallerVerifier(int uid){return false;}
    public android.content.ComponentName getComponentName(){return component;}
}
