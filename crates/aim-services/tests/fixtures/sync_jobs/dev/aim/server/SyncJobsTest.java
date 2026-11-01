package dev.aim.server;
import android.app.job.JobInfo;
import android.os.RemoteException;
import java.util.List;
public final class SyncJobsTest implements ISystemProcess {
    int[] counts = {3,2,1,6,5,4};
    boolean fail;
    String namespace;
    public List<JobInfo> getSystemScheduledOwnJobs(String namespace) throws RemoteException {
        if (fail) throw new RemoteException();
        this.namespace = namespace;
        return List.of();
    }
    public int[] getJobPersistStats() throws RemoteException {
        if (fail) throw new RemoteException();
        return counts;
    }
    public static void main(String[] args) {
        SyncJobsTest owner = new SyncJobsTest();
        NativeJobScheduler local = new NativeJobScheduler(owner);
        local.getSystemScheduledOwnJobs("SyncManager");
        if (!"SyncManager".equals(owner.namespace)) throw new AssertionError();
        local.getSystemScheduledOwnJobs(null);
        if (owner.namespace != null) throw new AssertionError();
        var s = local.getPersistStats();
        if (s.countAllJobsLoaded != 3 || s.countSystemServerJobsLoaded != 2
                || s.countSystemSyncManagerJobsLoaded != 1 || s.countAllJobsSaved != 6
                || s.countSystemServerJobsSaved != 5 || s.countSystemSyncManagerJobsSaved != 4) throw new AssertionError();
        owner.counts[0] = 99;
        if (s.countAllJobsLoaded != 3) throw new AssertionError("aliased snapshot");
        owner.counts = new int[0];
        try { local.getPersistStats(); throw new AssertionError(); } catch (IllegalStateException expected) {}
        owner.fail = true;
        try { local.getPersistStats(); throw new AssertionError(); } catch (IllegalStateException expected) {}
        try { local.getSystemScheduledOwnJobs(null); throw new AssertionError(); } catch (IllegalStateException expected) {}
        try { local.addBackingUpUid(1000); throw new AssertionError(); } catch (UnsupportedOperationException expected) {}
    }
}
