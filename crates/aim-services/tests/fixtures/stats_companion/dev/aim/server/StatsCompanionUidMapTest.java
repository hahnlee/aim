package dev.aim.server;

import android.content.*;
import android.content.pm.*;
import android.os.*;
import android.util.proto.ProtoOutputStream;
import static android.util.proto.ProtoStream.*;
import java.util.*;

/** StatsCompanionService's UID map and registered/dead receiver lifecycle at android-16.0.0_r1. */
public final class StatsCompanionUidMapTest {
    static void check(boolean ok, String message) { if(!ok) throw new AssertionError(message); }
    static final class Packages extends PackageManager {
        final List<Integer> queries=new ArrayList<>();
        final Map<String,PackageInfo> infos=new HashMap<>();
        final Map<String,InstallSourceInfo> sources=new HashMap<>();
        boolean failed, scanFailure;
        public List<PackageInfo> getInstalledPackagesAsUser(int flags,int user) {
            check(flags==(GET_SIGNING_CERTIFICATES|MATCH_UNINSTALLED_PACKAGES
                    |MATCH_ANY_USER|MATCH_STATIC_SHARED_AND_SDK_LIBRARIES), "package flags");
            if(scanFailure) throw new UnsupportedOperationException("published package owner unavailable");
            queries.add(user);
            return List.of(infos.get(user==0 ? "app" : "other"),new PackageInfo());
        }
        public List<PackageInfo> getInstalledPackages(int flags) {
            check(flags==MATCH_APEX,"apex flags");
            return List.of(infos.get("apex"),infos.get("app"));
        }
        public PackageInfo getPackageInfo(String name,int flags) throws NameNotFoundException {
            check(flags==(GET_SIGNING_CERTIFICATES|MATCH_ANY_USER),"incremental flags");
            if(failed) throw new NameNotFoundException(); return infos.get(name);
        }
        public InstallSourceInfo getInstallSourceInfo(String name) throws NameNotFoundException {
            if(!sources.containsKey(name)) throw new NameNotFoundException(); return sources.get(name);
        }
    }
    static final class Environment extends Context {
        final Packages packages=new Packages();
        final UserManager users=new UserManager();
        final Map<BroadcastReceiver,IntentFilter> receivers=new LinkedHashMap<>();
        int removed;
        public Object getSystemService(String name) { check(name.equals(USER_SERVICE),"user service"); return users; }
        public PackageManager getPackageManager() { return packages; }
        public Intent registerReceiverForAllUsers(BroadcastReceiver receiver,IntentFilter filter,
                String permission,Handler handler) {
            check(permission==null && handler!=null,"receiver registration");
            receivers.put(receiver,filter); return null;
        }
        public void unregisterReceiver(BroadcastReceiver receiver) {
            check(receivers.remove(receiver)!=null,"double unregister"); removed++;
        }
        void send(Intent intent) {
            for(var entry: new ArrayList<>(receivers.entrySet()))
                if(entry.getValue().actions.contains(intent.action)) entry.getKey().onReceive(this,intent);
        }
    }
    static final class Daemon implements IStatsd, IBinder {
        final List<DeathRecipient> deaths=new ArrayList<>();
        int maps,updates,removals,shutdowns;
        boolean dead;
        String name,versionName,installer;
        int uid;
        long version;
        byte[] hash;
        public IBinder asBinder() { return this; }
        public void linkToDeath(DeathRecipient recipient,int flags) throws RemoteException {
            if(dead) throw new RemoteException(); deaths.add(recipient);
        }
        public boolean unlinkToDeath(DeathRecipient recipient,int flags) { return deaths.remove(recipient); }
        public void informAllUidData(ParcelFileDescriptor fd) {
            check(ParcelFileDescriptor.open==2,"pipe not open during delivery");
            ProtoOutputStream.delivered=true; maps++;
        }
        public void informOnePackage(String name,int uid,long version,String versionName,String installer,byte[] hash) {
            updates++; this.name=name;this.uid=uid;this.version=version;
            this.versionName=versionName;this.installer=installer;this.hash=hash;
        }
        public void informOnePackageRemoved(String name,int uid) { removals++;this.name=name;this.uid=uid; }
        public void informDeviceShutdown() { shutdowns++; }
        void die() { dead=true; for(var recipient:List.copyOf(deaths)) recipient.binderDied(); }
    }
    static PackageInfo info(String name,int uid) {
        var pi=new PackageInfo(); pi.packageName=name;pi.applicationInfo=new ApplicationInfo();
        pi.applicationInfo.uid=uid; pi.version=0x100000002L;pi.versionName="version";
        return pi;
    }
    static Intent event(String action,boolean replacing) {
        var intent=new Intent(action); intent.replacing=replacing;
        intent.extras.uid=10123;intent.data.name="app";return intent;
    }
    static Object field(Map<Long,Object> record,long type,int number) {
        return record.get(type|FIELD_COUNT_SINGLE|number);
    }
    public static void main(String[] args) throws Exception {
        var env=new Environment(); var pm=env.packages;
        pm.infos.put("app",info("app",10123));pm.infos.put("other",info("other",1010123));
        pm.infos.put("apex",info("apex",1000));pm.infos.get("apex").isApex=true;
        var signing=new SigningInfo();
        signing.signers=new Signature[] {new Signature(new byte[]{9}),new Signature(new byte[]{-5})};
        pm.infos.get("app").signingInfo=signing;
        byte[] expected=java.security.MessageDigest.getInstance("SHA-256").digest(new byte[]{-5,9});
        var install=new InstallSourceInfo();install.initiating="com.android.shell";install.installing="store";
        pm.sources.put("app",install);
        var bridge=new StatsCompanionUidMap(env);var statsd=new Daemon();
        Binder.uid=10123;
        try { bridge.start(statsd); throw new AssertionError("non-system start"); }
        catch(SecurityException expectedError) { check(env.receivers.isEmpty(),"unauthorized registration"); }
        Binder.uid=1000;
        bridge.start(statsd);bridge.start(statsd);
        check(env.receivers.size()==3 && statsd.deaths.size()==1,"registration not idempotent");
        check(env.receivers.values().stream().filter(f->"package".equals(f.scheme)).count()==1,"package scheme");
        Handler.drain();
        check(statsd.maps==1 && pm.queries.equals(List.of(0,10)),"all user map");
        var rows=ProtoOutputStream.records;
        check(rows.size()==4,"active APEX per user; null application skipped");
        var row=rows.get(0);
        check(field(row,FIELD_TYPE_INT32,1).equals(10123),"uid");
        check(field(row,FIELD_TYPE_INT64,2).equals(0x100000002L),"long version");
        check(field(row,FIELD_TYPE_STRING,3).equals("version"),"version string");
        check(field(row,FIELD_TYPE_STRING,4).equals("app"),"package");
        check(field(row,FIELD_TYPE_STRING,5).equals("store"),"shell installer fallback");
        check(Arrays.equals((byte[])field(row,FIELD_TYPE_BYTES,6),expected),"sorted APK content signers SHA256");
        check(field(rows.get(1),FIELD_TYPE_STRING,5).equals(""),"missing installer");
        check(((byte[])field(rows.get(1),FIELD_TYPE_BYTES,6)).length==0,"missing signing info");
        check(ParcelFileDescriptor.open==0 && HandlerThread.active==1,"writer resource cleanup");
        for(String action:List.of(Intent.ACTION_PACKAGE_REMOVED,Intent.ACTION_PACKAGE_ADDED))
            env.send(event(action,true));
        check(statsd.updates==0 && statsd.removals==0,"replacement intermediate broadcasts");
        pm.infos.get("app").versionName=null;
        install.initiating="initiator";
        env.send(event(Intent.ACTION_PACKAGE_REPLACED,true));
        check(statsd.updates==1 && statsd.name.equals("app") && statsd.uid==10123
                && statsd.version==0x100000002L && statsd.versionName.equals("")
                && statsd.installer.equals("initiator") && Arrays.equals(statsd.hash,expected),"package replacement");
        install.initiating=null;
        env.send(event(Intent.ACTION_PACKAGE_ADDED,false));
        check(statsd.updates==2 && statsd.installer.equals("store"),"normal addition");
        pm.failed=true;env.send(event(Intent.ACTION_PACKAGE_ADDED,false));pm.failed=false;
        check(statsd.updates==2,"missing package must not fabricate an update");
        env.send(event(Intent.ACTION_PACKAGE_REMOVED,false));
        check(statsd.removals==1 && statsd.name.equals("app") && statsd.uid==10123,"removal");
        for(String action:List.of(Intent.ACTION_USER_INITIALIZE,Intent.ACTION_USER_REMOVED)) {
            env.send(new Intent(action));Handler.drain();
        }
        check(statsd.maps==3,"user map refreshes");
        env.send(new Intent(Intent.ACTION_SHUTDOWN));
        check(statsd.shutdowns==0,"background shutdown ignored");
        var shutdown=new Intent(Intent.ACTION_SHUTDOWN);shutdown.flags=Intent.FLAG_RECEIVER_FOREGROUND;
        env.send(shutdown);env.send(new Intent(Intent.ACTION_REBOOT));
        check(statsd.shutdowns==2,"shutdown and reboot");
        var oldDeath=statsd.deaths.get(0);statsd.die();oldDeath.binderDied();
        check(env.receivers.isEmpty() && env.removed==3 && HandlerThread.active==0,"death unregistration exactly once");
        var replacement=new Daemon();bridge.start(replacement);Handler.drain();oldDeath.binderDied();
        check(env.receivers.size()==3 && replacement.maps==1,"late death removed new receivers");
        var stale=new Daemon();stale.dead=true;
        try { bridge.start(stale);throw new AssertionError("stale start succeeded"); }
        catch(RemoteException expectedError) {}
        check(env.receivers.size()==3 && HandlerThread.active==1,"stale start replaced live cohort");
        replacement.die();
        var alreadyDead=new Daemon();alreadyDead.dead=true;
        try { bridge.start(alreadyDead);throw new AssertionError("dead statsd linked"); }
        catch(RemoteException expectedError) {}
        check(env.receivers.isEmpty() && HandlerThread.active==0,"link failure cleanup");
        var earlyDeath=new Daemon();bridge.start(earlyDeath);earlyDeath.die();Handler.drain();
        check(earlyDeath.maps==0 && ParcelFileDescriptor.open==0 && HandlerThread.active==0,"death before writer cleanup");
        var scanFailure=new Daemon();pm.scanFailure=true;bridge.start(scanFailure);
        try { Handler.drain();throw new AssertionError("scan failure swallowed"); }
        catch(UnsupportedOperationException expectedError) {}
        check(ParcelFileDescriptor.open==0 && HandlerThread.active==1,"failed writer cleanup");
        pm.scanFailure=false;scanFailure.die();
        System.out.println("UID map, package/user/shutdown receivers, permissions and death cleanup passed");
    }
}
