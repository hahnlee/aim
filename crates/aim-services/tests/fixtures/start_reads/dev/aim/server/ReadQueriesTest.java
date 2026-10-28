package dev.aim.server;
import android.content.*;
import android.content.pm.RegisteredServicesCache.ServiceInfo;
import android.media.audiopolicy.AudioProductStrategy;
import android.os.Binder;
import android.os.Parcel;

interface IAudioPolicyQueries {
    abstract class Stub { public abstract byte[] getAudioProductStrategies(); }
}
interface ISyncAdapterQueries {
    abstract class Stub { public abstract byte[] getSyncAdapterTypes(int uid, int user) throws android.os.RemoteException; }
}
interface ISystemProcess { boolean filterSyncAdapterPackageAccess(String name, int uid, int user); }
public class ReadQueriesTest {
    private static void check(boolean condition) { if (!condition) throw new AssertionError(); }
    private static void denied(Runnable r) {
        try { r.run(); throw new AssertionError("accepted app uid"); }
        catch (SecurityException expected) {}
    }
    public static void main(String[] args) throws android.os.RemoteException {
        var audio = new AudioPolicyQueries();
        var sync = new SyncAdapterQueries(new Context(), (name, uid, user) -> {
            check(uid == 10100 && user == 10);
            return name.equals("hidden");
        });
        Binder.uid = 10100;
        denied(() -> audio.getAudioProductStrategies());
        try { sync.getSyncAdapterTypes(10100, 10); throw new AssertionError("accepted app uid"); }
        catch (SecurityException expected) {}
        check(SyncAdaptersCache.constructed == 0 && Parcel.recycled == 0);
        Binder.uid = 1000;
        var strategy = new AudioProductStrategy();
        AudioProductStrategy.strategies.add(strategy);
        check(audio.getAudioProductStrategies()[0] == 1 && Parcel.written[0] == strategy);
        AudioProductStrategy.strategies.clear();
        check(audio.getAudioProductStrategies()[0] == 0);
        var visible = new SyncAdapterType("visible");
        var legacy = new SyncAdapterType(null);
        SyncAdaptersCache.services.add(new ServiceInfo<>(visible));
        SyncAdaptersCache.services.add(new ServiceInfo<>(new SyncAdapterType("hidden")));
        SyncAdaptersCache.services.add(new ServiceInfo<>(legacy));
        check(sync.getSyncAdapterTypes(10100, 10)[0] == 2);
        check(Parcel.written[0] == visible && Parcel.written[1] == legacy);
        SyncAdaptersCache.services.clear();
        check(sync.getSyncAdapterTypes(10100, 10)[0] == 0);
        check(SyncAdaptersCache.constructed == 1 && SyncAdaptersCache.user == 10);
        SyncAdaptersCache.fail = true;
        try { sync.getSyncAdapterTypes(10100, 10); throw new AssertionError(); }
        catch (IllegalStateException expected) { check(expected.getMessage().equals("cache failure")); }
        check(Binder.cleared == 0 && Parcel.recycled == 5);
    }
}
