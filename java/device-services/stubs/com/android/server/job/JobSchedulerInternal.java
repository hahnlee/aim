// Compile-only pinned image declarations; device-services verifies linkage.
package com.android.server.job;
import android.app.job.JobInfo;
import java.util.List;
public interface JobSchedulerInternal {
    List<JobInfo> getSystemScheduledOwnJobs(String namespace);
    JobStorePersistStats getPersistStats();
    void cancelJobsForUid(int uid, boolean proxy, int reason, int internal, String debug);
    void addBackingUpUid(int uid);
    void removeBackingUpUid(int uid);
    void clearAllBackingUpUids();
    String getCloudMediaProviderPackage(int user);
    void reportAppUsage(String name, int user);
    boolean isAppConsideredBuggy(int user, String name, int blameUser, String blameName);
    boolean isNotificationAssociatedWithAnyUserInitiatedJobs(int id, int user, String name);
    boolean isNotificationChannelAssociatedWithAnyUserInitiatedJobs(String channel, int user, String name);
    public class JobStorePersistStats {
        public int countAllJobsLoaded, countSystemServerJobsLoaded, countSystemSyncManagerJobsLoaded;
        public int countAllJobsSaved, countSystemServerJobsSaved, countSystemSyncManagerJobsSaved;
        public JobStorePersistStats() { throw new RuntimeException("stub"); }
    }
}
