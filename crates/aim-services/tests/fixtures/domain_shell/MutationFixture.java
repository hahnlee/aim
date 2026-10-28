package com.android.server.pm.verify.domain;
import android.util.*;
import android.os.Binder;
import java.util.*;

/** Runtime boundaries for executing unmodified pinned service mutation methods. */
class MutationFixture {
    static final class AndroidPackage {
        final ArraySet<String> hosts=new ArraySet<>(List.of("a.test","b.test"));
    }
    record PackageStateInternal(String getPackageName, AndroidPackage getPkg, boolean isSystem) {}
    static final class Computer {
        final Map<String,PackageStateInternal> packages=new HashMap<>();
        PackageStateInternal getPackageStateInternal(String name){return packages.get(name);}
    }
    static final class Connection {
        final Computer snapshot=new Computer(); int writes;
        int getCallingUid(){return Binder.getCallingUid();}
        Computer snapshot(){return snapshot;}
        void scheduleWriteSettings(){writes++;}
    }
    static final class Collector {
        ArraySet<String> collectValidAutoVerifyDomains(AndroidPackage p){return new ArraySet<>(p.hosts);}
    }
    static final class Config {
        final Set<String> linked=new HashSet<>();
        Set<String> getLinkedApps(){return linked;}
    }
    static final class DomainVerificationInternalUserState {
        final boolean allowed; final ArraySet<String> hosts=new ArraySet<>(List.of("a.test","b.test","web.test"));
        DomainVerificationInternalUserState(boolean allowed){this.allowed=allowed;}
        boolean isLinkHandlingAllowed(){return allowed;}
        void removeHost(String host){hosts.remove(host);}
    }
    static final class DomainVerificationPkgState {
        final String name; final ArrayMap<String,Integer> states=new ArrayMap<>();
        final SparseArray<DomainVerificationInternalUserState> users=new SparseArray<>();
        DomainVerificationPkgState(String name){this.name=name;}
        String getPackageName(){return name;}
        ArrayMap<String,Integer> getStateMap(){return states;}
        SparseArray<DomainVerificationInternalUserState> getUserStates(){return users;}
        void removeAllUsers(){users.clear();}
        void removeUser(int user){users.remove(user);}
    }
    final Object mLock=new Object();
    final Connection mConnection=new Connection();
    final Collector mCollector=new Collector();
    final Config mSystemConfig=new Config();
    final DomainVerificationEnforcer mEnforcer=new DomainVerificationEnforcer(null);
    final ArrayMap<String,DomainVerificationPkgState> mAttachedPkgStates=new ArrayMap<>();
    static OriginalDomainMutations fixture(){
        var s=new OriginalDomainMutations();
        for(String name:List.of("p","q")){
            var p=new DomainVerificationPkgState(name);
            p.states.put("a.test",7);p.states.put("b.test",1024);
            p.users.put(0,new DomainVerificationInternalUserState(true));
            p.users.put(10,new DomainVerificationInternalUserState(false));
            s.mAttachedPkgStates.put(name,p);
            s.mConnection.snapshot.packages.put(name,new PackageStateInternal(name,new AndroidPackage(),false));
        }
        return s;
    }
    static void require(boolean value){if(!value)throw new AssertionError();}
    public static void main(String[] ignored)throws Exception {
        for(int state=0;state<=3;state++)for(boolean named:new boolean[]{false,true}){
            var s=fixture();
            s.setDomainVerificationStatusInternal(named?"p":null,state,new ArraySet<>(List.of("a.test","b.test","invalid.test")));
            require(s.mConnection.writes==1);
            for(String name:List.of("p","q")){
                var p=s.mAttachedPkgStates.get(name);
                require(p.users.get(0).hosts.contains("b.test")==!(named&&(state==1||state==2)));
                require(p.users.get(0).hosts.contains("a.test"));
                require(p.users.get(10).hosts.contains("b.test"));
                if(name.equals("p")||!named)require(p.states.equals(Map.of("a.test",state,"b.test",state)));
            }
        }
        for(boolean immutable:new boolean[]{false,true}){
            var s=fixture();var p=s.mAttachedPkgStates.get("p");p.states.clear();
            for(int state:new int[]{0,1,2,3,4,5,6,7,8,1023,1024,2048})p.states.put("h"+state+".test",state);
            if(immutable){s.mSystemConfig.linked.add("p");s.mConnection.snapshot.packages.put("p",new PackageStateInternal("p",new AndroidPackage(),true));}
            s.clearDomainVerificationState(List.of("p"));
            for(int state:new int[]{0,2,3,4,6,8,1023})require(p.states.get("h"+state+".test")==state);
            for(int state:new int[]{1,5,1024,2048})require(!p.states.containsKey("h"+state+".test"));
            require(p.states.containsKey("h7.test")==immutable);
            require(p.states.containsKey("a.test")==immutable);
            require(p.users.size()==2);
            s.clearUserStates(List.of("p"),0);require(p.users.size()==1&&p.users.get(10)!=null);
            s.clearUserStates(null,-1);require(p.users.size()==0&&s.mAttachedPkgStates.get("q").users.size()==0);
        }
        var absent=fixture();absent.mConnection.snapshot.packages.remove("q");
        absent.setDomainVerificationStatusInternal(null,0,null);
        require(absent.mAttachedPkgStates.get("q").states.get("b.test")==1024);
        absent.clearDomainVerificationState(List.of("missing","q"));
        try{absent.setDomainVerificationStatusInternal("q",1,null);throw new AssertionError();}
        catch(android.content.pm.PackageManager.NameNotFoundException expected){}
        try{absent.setDomainVerificationStatusInternal("p",4,null);throw new AssertionError();}
        catch(IllegalArgumentException expected){require(expected.getMessage().equals("State must be one of NO_RESPONSE, SUCCESS, APPROVED, or DENIED"));}
        var s=fixture();
        try{s.clearUserStates(List.of("p","missing"),-1);throw new AssertionError();}
        catch(NullPointerException expected){require(s.mAttachedPkgStates.get("p").users.size()==0&&s.mConnection.writes==0);}
    }
}
