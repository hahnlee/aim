import android.content.*;
import android.content.pm.ResolveInfo;
import android.content.pm.verify.domain.DomainVerificationRequest;
import android.os.*;
import com.android.server.pm.*;
import com.android.server.pm.verify.domain.*;
import com.android.server.pm.verify.domain.proxy.*;
import dev.aim.server.PackageBootstrapBridge;
import java.util.*;

/** Production bridge/selection/connection with unchanged pinned proxy and request sources.
 * Context, Handler, package queries, Binder and allowlisting are recorded JVM boundaries. */
public final class DomainDeliveryTest {
    static void require(boolean value){if(!value)throw new AssertionError();}
    static void failure(Runnable task,String message){
        try{task.run();throw new AssertionError("lost failure: "+message);}
        catch(RuntimeException error){require(error.getMessage().equals(message));}
    }
    static NativeDomainVerificationConnection install(DomainVerificationService owner,
            NativeComputer computer,Context context,Handler handler){
        var connection=new NativeDomainVerificationConnection(owner,()->computer,new UserManagerInternal(),new NativeDomainSettings(),handler);
        connection.installProxy(context);return connection;
    }
    public static void main(String[] args){
        var owner=new DomainVerificationService();var computer=new NativeComputer();
        computer.modern.add(new ResolveInfo("denied",100));computer.denied.add("denied");
        computer.modern.add(new ResolveInfo("disabled",90));computer.disabled.add("disabled");
        computer.modern.add(new ResolveInfo("lower",1));
        computer.modern.add(new ResolveInfo("agent",10));computer.modern.add(new ResolveInfo("tie",10));
        var context=new Context();var handler=new Handler();
        var connection=install(owner,computer,context,handler);
        require(computer.closes==1);
        require(owner.getProxy().getComponentName().equals(new ComponentName("agent","Receiver")));
        require(computer.checks.equals(List.of(
                "android.permission.DOMAIN_VERIFICATION_AGENT:denied:0",
                "android.permission.DOMAIN_VERIFICATION_AGENT:disabled:0",
                "android.permission.DOMAIN_VERIFICATION_AGENT:lower:0",
                "android.permission.DOMAIN_VERIFICATION_AGENT:agent:0",
                "android.permission.DOMAIN_VERIFICATION_AGENT:tie:0")));
        require(owner.getProxy().isCallerVerifier(10123));require(!owner.getProxy().isCallerVerifier(10124));
        var bridge=new PackageBootstrapBridge(owner);
        Binder.uid=2000;
        failure(()->bridge.sendDomainVerificationRequest(List.of("p")),"the package bridge serves the system uid only");
        require(handler.queue.isEmpty());Binder.uid=1000;
        bridge.sendDomainVerificationRequest(List.of("p","q","p"));
        require(context.broadcasts.isEmpty()&&handler.queue.size()==1);handler.drain();
        require(context.broadcasts.size()==1);
        var broadcast=context.broadcasts.get(0);var intent=broadcast.intent();
        require(intent.action.equals("android.intent.action.DOMAINS_NEED_VERIFICATION"));
        require(intent.component.equals(new ComponentName("agent","Receiver")));
        require(intent.flags==0x10000000&&broadcast.user().equals(UserHandle.SYSTEM));
        // The original V2 broadcast uses null receiverPermission; selection checked AGENT above.
        require(broadcast.permission()==null);
        require(intent.extras.keySet().equals(Set.of("android.content.pm.verify.domain.extra.VERIFICATION_REQUEST")));
        var request=(DomainVerificationRequest)intent.extras.values().iterator().next();
        require(request.getPackageNames().equals(Set.of("p","q")));
        var options=broadcast.options();
        require(options.duration==12345&&options.type==0&&options.reason==308&&options.tag.equals(""));
        var idle=com.android.server.LocalServices.idle;
        require(idle.calls.equals(List.of(new com.android.server.DeviceIdleInternal.Allowlist(
                1000,"agent",12345,0,true,308,"domain verification agent"))));
        handler.accepting=false;
        failure(()->bridge.sendDomainVerificationRequest(List.of("p")),"domain verifier Handler stopped");
        handler.accepting=true;
        context.failure=new IllegalStateException("broadcast failed");
        bridge.sendDomainVerificationRequest(List.of("p"));failure(handler::drain,"broadcast failed");
        require(context.broadcasts.size()==1);
        failure(()->new PackageBootstrapBridge(null).sendDomainVerificationRequest(List.of("p")),"domain owner is unavailable");
        failure(()->new PackageBootstrapBridge(new DomainVerificationService()).sendDomainVerificationRequest(List.of("p")),"domain proxy is unavailable");
        failure(()->connection.installProxy(context),"domain verifier proxy already installed");
        // No installed/eligible verifier: retain the original unavailable proxy, no delivery or error.
        for(boolean ineligible:new boolean[]{false,true}){
            var absent=new DomainVerificationService();var none=new NativeComputer();
            if(ineligible){none.modern.add(new ResolveInfo("denied",100));none.denied.add("denied");}
            var queue=new Handler();var noContext=new Context();install(absent,none,noContext,queue);
            new PackageBootstrapBridge(absent).sendDomainVerificationRequest(List.of("p"));
            require(absent.getProxy() instanceof DomainVerificationProxyUnavailable);
            require(absent.getProxy().getComponentName()==null&&!absent.getProxy().isCallerVerifier(10123));
            require(!absent.runMessage(1,Set.of("p"))&&queue.queue.isEmpty()&&noContext.broadcasts.isEmpty());
        }
        // Exercise the real factory/combined policy; only legacy transport itself is doubled.
        for(boolean samePackage:new boolean[]{false,true}){
            var combined=new DomainVerificationService();var both=new NativeComputer();
            both.modern.add(new ResolveInfo("agent",1));both.legacy.add(new ResolveInfo(samePackage?"agent":"legacy",2));
            var queue=new Handler();var sink=new Context();install(combined,both,sink,queue);
            DomainVerificationProxyV1.sends=0;
            new PackageBootstrapBridge(combined).sendDomainVerificationRequest(List.of("p"));queue.drain();
            require(DomainVerificationProxyV1.sends==(samePackage?1:0)&&sink.broadcasts.size()==1);
            require(both.checks.get(0).equals("android.permission.INTENT_FILTER_VERIFICATION_AGENT:"+(samePackage?"agent":"legacy")+":0"));
        }
        System.out.println("domain delivery: selection, original V2 request/broadcast, unavailable verifier, authorization and failures passed");
    }
}
