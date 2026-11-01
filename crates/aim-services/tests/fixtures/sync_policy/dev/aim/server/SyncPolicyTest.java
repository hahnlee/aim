package dev.aim.server;
import android.os.RemoteException;
import java.util.Arrays;
public final class SyncPolicyTest implements ISystemProcess {
    Object[] call;
    boolean fail;
    void record(Object... values) throws RemoteException {
        if (fail) throw new RemoteException();
        call = values;
    }
    void expect(Object... values) {
        if (!Arrays.equals(call, values)) throw new AssertionError(Arrays.toString(call));
    }
    public int getSyncAdapterStandbyBucket(String name, int user) throws RemoteException { record(name, user); return 40; }
    public void reportSyncScheduled(String name, int user, boolean exempted) throws RemoteException { record(name, user, exempted); }
    public void reportExemptedSyncStart(String name, int user) throws RemoteException { record(name, user); }
    public void addSyncTempAllowlist(int caller, String name, long duration, int type, int user, boolean sync, int code, String reason) throws RemoteException { record(caller, name, duration, type, user, sync, code, reason); }
    public static void main(String[] args) {
        SyncPolicyTest owner = new SyncPolicyTest();
        var usage = new SyncPolicyLocals.Usage(owner);
        var idle = new SyncPolicyLocals.Idle(owner);
        if (usage.getAppStandbyBucket("p", 7, 123) != 40) throw new AssertionError();
        owner.expect("p", 7);
        for (boolean exempted : new boolean[]{true, false}) {
            usage.reportSyncScheduled("p", 7, exempted); owner.expect("p", 7, exempted);
        }
        usage.reportExemptedSyncStart("p", 7); owner.expect("p", 7);
        idle.addPowerSaveTempWhitelistApp(1000, "p", 123L, 1, 7, false, 306, "sync by top app");
        owner.expect(1000, "p", 123L, 1, 7, false, 306, "sync by top app");
        idle.addPowerSaveTempWhitelistApp(1000, "p", 123L, 7, false, 306, null);
        owner.expect(1000, "p", 123L, 0, 7, false, 306, null);
        owner.fail = true;
        for (Runnable call : new Runnable[]{() -> usage.getAppStandbyBucket("p", 0, 0),
                () -> usage.reportSyncScheduled("p", 0, false), () -> usage.reportExemptedSyncStart("p", 0),
                () -> idle.addPowerSaveTempWhitelistApp(1000, "p", 1, 1, 0, false, 306, "r")}) {
            try { call.run(); throw new AssertionError(); } catch (IllegalStateException expected) {}
        }
        try { usage.prepareShutdown(); throw new AssertionError(); } catch (UnsupportedOperationException expected) {}
        try { idle.setJobsActive(true); throw new AssertionError(); } catch (UnsupportedOperationException expected) {}
    }
}
