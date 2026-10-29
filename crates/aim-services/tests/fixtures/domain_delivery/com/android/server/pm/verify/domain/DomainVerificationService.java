package com.android.server.pm.verify.domain;
public final class DomainVerificationService implements DomainVerificationManagerInternal {
    public com.android.server.pm.verify.domain.proxy.DomainVerificationProxy proxy;
    public void setProxy(com.android.server.pm.verify.domain.proxy.DomainVerificationProxy value){proxy=value;}
    public com.android.server.pm.verify.domain.proxy.DomainVerificationProxy getProxy(){return proxy;}
    public DomainVerificationCollector getCollector(){return null;}
    public java.util.UUID generateNewId(){throw new AssertionError("unexpected state mutation");}
    public boolean runMessage(int code,Object value){return proxy.runMessage(code,value);}
}
