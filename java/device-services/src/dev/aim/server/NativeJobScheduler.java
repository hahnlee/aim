package dev.aim.server;

import android.app.job.JobInfo;
import android.os.RemoteException;
import com.android.server.LocalServices;
import com.android.server.job.JobSchedulerInternal;
import java.util.List;
import java.util.Objects;

/** Local reads used by the retained SyncManager; the scheduler remains native (#1460). */
final class NativeJobScheduler implements JobSchedulerInternal {
    private final ISystemProcess owner;
    NativeJobScheduler(ISystemProcess owner) { this.owner = Objects.requireNonNull(owner); }
    static void publish(ISystemProcess owner) {
        LocalServices.addService(JobSchedulerInternal.class, new NativeJobScheduler(owner));
    }
    @Override public List<JobInfo> getSystemScheduledOwnJobs(String namespace) {
        try { return Objects.requireNonNull(owner.getSystemScheduledOwnJobs(namespace)); }
        catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
    @Override public JobStorePersistStats getPersistStats() {
        try {
            int[] counts = Objects.requireNonNull(owner.getJobPersistStats());
            if (counts.length != 6) throw new IllegalStateException("invalid job persist stats");
            JobStorePersistStats result = new JobStorePersistStats();
            result.countAllJobsLoaded = counts[0];
            result.countSystemServerJobsLoaded = counts[1];
            result.countSystemSyncManagerJobsLoaded = counts[2];
            result.countAllJobsSaved = counts[3];
            result.countSystemServerJobsSaved = counts[4];
            result.countSystemSyncManagerJobsSaved = counts[5];
            return result;
        } catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
    private UnsupportedOperationException unsupported() {
        return new UnsupportedOperationException("JobSchedulerInternal operation (#1490)");
    }
    @Override public void cancelJobsForUid(int uid, boolean proxy, int reason, int internal, String debug) { throw unsupported(); }
    @Override public void addBackingUpUid(int uid) { throw unsupported(); }
    @Override public void removeBackingUpUid(int uid) { throw unsupported(); }
    @Override public void clearAllBackingUpUids() { throw unsupported(); }
    @Override public String getCloudMediaProviderPackage(int user) { throw unsupported(); }
    @Override public void reportAppUsage(String name, int user) { throw unsupported(); }
    @Override public boolean isAppConsideredBuggy(int user, String name, int blameUser, String blameName) { throw unsupported(); }
    @Override public boolean isNotificationAssociatedWithAnyUserInitiatedJobs(int id, int user, String name) { throw unsupported(); }
    @Override public boolean isNotificationChannelAssociatedWithAnyUserInitiatedJobs(String channel, int user, String name) { throw unsupported(); }
}
