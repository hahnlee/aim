package com.android.server.pm;

import android.content.Context;
import android.content.Intent;
import android.content.ComponentName;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.os.Binder;
import com.android.server.pm.verify.domain.proxy.DomainVerificationProxy;
import com.android.server.pm.verify.domain.proxy.DomainVerificationProxyV1;
import com.android.server.pm.verify.domain.proxy.DomainVerificationProxyV2;
import android.os.Handler;
import android.os.UserHandle;
import com.android.server.pm.verify.domain.DomainVerificationManagerInternal;
import com.android.server.pm.verify.domain.DomainVerificationService;
import java.util.Objects;
import java.util.function.Supplier;

/** Original DomainVerificationConnection contract over the native package owner. */
public final class NativeDomainVerificationConnection implements DomainVerificationManagerInternal.Connection,
        DomainVerificationProxyV1.Connection, DomainVerificationProxyV2.Connection {
    private final Supplier<NativeComputer> computers;
    private final UserManagerInternal users;
    private final NativeDomainSettings persistence;
    private final DomainVerificationService domains;
    private final Handler handler;
    private Context context;
    public NativeDomainVerificationConnection(DomainVerificationService domains,
            Supplier<NativeComputer> computers,UserManagerInternal users,
            NativeDomainSettings persistence,Handler handler) {
        this.domains=Objects.requireNonNull(domains);this.computers=Objects.requireNonNull(computers);
        this.users=Objects.requireNonNull(users);this.persistence=Objects.requireNonNull(persistence);
        this.handler=Objects.requireNonNull(handler);
    }
    /** Same post-scan proxy selection and factory as original PMS. */
    public synchronized void installProxy(Context context) {
        if(this.context!=null)throw new IllegalStateException("domain verifier proxy already installed");
        this.context=Objects.requireNonNull(context);
        try(var snapshot=computers.get()) {
            ComponentName legacy=select(snapshot,"android.intent.action.INTENT_FILTER_NEEDS_VERIFICATION",
                    "application/vnd.android.package-archive","android.permission.INTENT_FILTER_VERIFICATION_AGENT",false);
            ComponentName modern=select(snapshot,"android.intent.action.DOMAINS_NEED_VERIFICATION",
                    null,"android.permission.DOMAIN_VERIFICATION_AGENT",true);
            domains.setProxy(DomainVerificationProxy.makeProxy(legacy,modern,context,domains,domains.getCollector(),this));
        }
    }
    private static ComponentName select(NativeComputer snapshot,String action,String type,String permission,boolean enabled) {
        long flags=0x100000L | PackageManager.MATCH_DIRECT_BOOT_AWARE | PackageManager.MATCH_DIRECT_BOOT_UNAWARE;
        ResolveInfo best=null;
        for(var candidate:snapshot.queryDomainVerificationReceivers(new Intent(action),type,flags,0)) {
            var component=candidate.getComponentInfo();
            if(snapshot.checkDomainVerificationPermission(permission,component.packageName,0)!=PackageManager.PERMISSION_GRANTED)continue;
            if(best==null||candidate.priority>best.priority) {
                if(!enabled||snapshot.isComponentEffectivelyEnabled(component,UserHandle.of(0)))best=candidate;
            }
        }
        return best==null?null:best.getComponentInfo().getComponentName();
    }
    @Override public void schedule(int code,Object object) {
        if(!handler.post(()->domains.runMessage(code,object)))throw new IllegalStateException("domain verifier Handler stopped");
    }
    @Override public long getPowerSaveTempWhitelistAppDuration() {
        Context current;
        synchronized(this){current=Objects.requireNonNull(context,"domain verifier proxy not installed");}
        return VerificationUtils.getDefaultVerificationTimeout(current);
    }
    @Override public com.android.server.DeviceIdleInternal getDeviceIdleInternal() {
        return Objects.requireNonNull(com.android.server.LocalServices.getService(com.android.server.DeviceIdleInternal.class));
    }
    @Override public boolean isCallerPackage(int uid,String name) {
        try(var snapshot=computers.get()){return uid==snapshot.getPackageUid(name,0,UserHandle.getUserId(uid));}
    }
    @Override public com.android.server.pm.pkg.AndroidPackage getPackage(String name) {
        try(var snapshot=computers.get()){return snapshot.getPackage(name);}
    }
    @Override public int getCallingUid(){return Binder.getCallingUid();}
    @Override public int getCallingUserId(){return UserHandle.getUserId(Binder.getCallingUid());}
    @Override public int[] getAllUserIds(){return users.getUserIds();}
    @Override public boolean doesUserExist(int user){return users.exists(user);}
    @Override public Computer snapshot(){return Objects.requireNonNull(computers.get());}
    @Override public boolean filterAppAccess(String name,int uid,int user){
        try(var snapshot=computers.get()){return snapshot.filterAppAccess(name,uid,user,true);}
    }
    @Override public void scheduleWriteSettings(){persistence.scheduleWrite();}
}
