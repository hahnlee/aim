package dev.darwinart.runtime.job;

import android.app.job.JobInfo;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** Injectable wire-test stand-in; production state is tested separately. */
public final class JobSchedulerService {
    public static final int RESULT_FAILURE = 0;
    public static final int RESULT_SUCCESS = 1;
    private final HashMap<String, List<JobInfo>> pending = new HashMap<>();

    public int schedule(int pid, String namespace, JobInfo job) {
        if (job == null || job.getService() == null
                || !"org.example.app".equals(job.getService().getPackageName())) {
            throw new SecurityException("foreign job service");
        }
        pending.computeIfAbsent(namespace, unused -> new ArrayList<>()).add(job);
        return RESULT_SUCCESS;
    }
    public void cancel(int pid, String namespace, int id) { pending.remove(namespace); }
    public void cancelAll(int pid) { pending.clear(); }
    public void cancelAll(int pid, String namespace) { pending.remove(namespace); }
    public Map<String, List<JobInfo>> allPending(int pid) { return pending; }
    public List<JobInfo> pending(int pid, String namespace) {
        return pending.getOrDefault(namespace, new ArrayList<>());
    }
    public JobInfo pending(int pid, String namespace, int id) {
        List<JobInfo> jobs = pending(pid, namespace);
        return jobs.isEmpty() ? null : jobs.get(0);
    }
}
