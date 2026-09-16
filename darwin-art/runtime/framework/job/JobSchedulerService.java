package dev.darwinart.runtime.job;

import android.Manifest;
import android.app.job.JobInfo;
import android.content.pm.ServiceInfo;
import android.net.Network;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import dev.darwinart.runtime.am.SystemServiceBindings;
import dev.darwinart.runtime.connectivity.ConnectivityProjection;
import dev.darwinart.runtime.connectivity.ConnectivitySnapshot;
import dev.darwinart.runtime.connectivity.ConnectivityState;
import dev.darwinart.runtime.pm.InstalledServiceInfo;
import dev.darwinart.runtime.pm.PackageRecords;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;

/** Android-owned admission, constraint and execution state for scheduled jobs. */
public final class JobSchedulerService implements JobServiceContext.Listener {
    public static final int RESULT_FAILURE = 0;
    public static final int RESULT_SUCCESS = 1;
    private static final long EXECUTION_HANDSHAKE_TIMEOUT_MS = 8_000;

    private static final class Key {
        final int uid;
        final String namespace;
        final int id;

        Key(int ownerUid, String jobNamespace, int jobId) {
            uid = ownerUid;
            namespace = jobNamespace;
            id = jobId;
        }

        @Override public boolean equals(Object other) {
            if (!(other instanceof Key)) return false;
            Key key = (Key) other;
            return uid == key.uid && id == key.id
                    && (namespace == null ? key.namespace == null : namespace.equals(key.namespace));
        }

        @Override public int hashCode() {
            return 31 * (31 * uid + id) + (namespace == null ? 0 : namespace.hashCode());
        }
    }

    private final PackageRecords.Source packages;
    private final ApplicationProcessRegistry processes;
    private final SystemServiceBindings bindings;
    private final ConnectivityState connectivity;
    private final ConnectivityProjection connectivityProjection = new ConnectivityProjection();
    private final HashMap<Key, JobRecord> jobs = new HashMap<>();
    private final ScheduledExecutorService dispatcher = Executors.newSingleThreadScheduledExecutor(
            runnable -> {
                Thread thread = new Thread(runnable, "JobScheduler");
                thread.setDaemon(true);
                return thread;
            });
    private long nextGeneration = 1;

    public JobSchedulerService(PackageRecords.Source packageSource,
            ApplicationProcessRegistry processRegistry, SystemServiceBindings serviceBindings,
            ConnectivityState connectivityState) {
        packages = packageSource;
        processes = processRegistry;
        bindings = serviceBindings;
        connectivity = connectivityState;
    }

    public synchronized int schedule(int callingPid, String namespace, JobInfo job) {
        ApplicationProcessRegistry.AttachedApplication caller =
                processes.requireAttachedProcess(callingPid);
        if (!isSupported(job)) return RESULT_FAILURE;
        requireOwnedJob(caller, job);
        JobRecord record = new JobRecord(caller.uid, caller.packageName, namespace, job,
                nextGeneration());
        Key key = key(record);
        JobRecord replaced = jobs.put(key, record);
        if (replaced != null && replaced.execution != null) replaced.execution.stop();
        dispatcher.execute(() -> maybeStart(key, record.generation));
        return RESULT_SUCCESS;
    }

    public synchronized void cancel(int callingPid, String namespace, int jobId) {
        int uid = processes.requireAttachedProcess(callingPid).uid;
        remove(new Key(uid, namespace, jobId));
    }

    public synchronized void cancelAll(int callingPid) {
        int uid = processes.requireAttachedProcess(callingPid).uid;
        for (Key key : new ArrayList<>(jobs.keySet())) if (key.uid == uid) remove(key);
    }

    public synchronized void cancelAll(int callingPid, String namespace) {
        int uid = processes.requireAttachedProcess(callingPid).uid;
        for (Key key : new ArrayList<>(jobs.keySet())) {
            if (key.uid == uid && equal(namespace, key.namespace)) remove(key);
        }
    }

    public synchronized Map<String, List<JobInfo>> allPending(int callingPid) {
        int uid = processes.requireAttachedProcess(callingPid).uid;
        HashMap<String, List<JobInfo>> result = new HashMap<>();
        for (JobRecord record : jobs.values()) {
            if (record.uid == uid) {
                result.computeIfAbsent(record.namespace, unused -> new ArrayList<>()).add(record.job);
            }
        }
        return result;
    }

    public synchronized List<JobInfo> pending(int callingPid, String namespace) {
        int uid = processes.requireAttachedProcess(callingPid).uid;
        ArrayList<JobInfo> result = new ArrayList<>();
        for (JobRecord record : jobs.values()) {
            if (record.uid == uid && equal(namespace, record.namespace)) result.add(record.job);
        }
        return result;
    }

    public synchronized JobInfo pending(int callingPid, String namespace, int jobId) {
        int uid = processes.requireAttachedProcess(callingPid).uid;
        JobRecord record = jobs.get(new Key(uid, namespace, jobId));
        return record == null ? null : record.job;
    }

    @Override
    public synchronized void onStartAcknowledged(JobServiceContext context, boolean ongoing) {
        JobRecord record = current(context);
        if (record == null) return;
        if (ongoing) {
            record.state = JobRecord.State.RUNNING;
        } else {
            finish(record, false);
        }
    }

    @Override
    public synchronized void onFinished(JobServiceContext context, boolean reschedule) {
        JobRecord record = current(context);
        if (record != null) finish(record, reschedule);
    }

    @Override
    public synchronized void onExecutionFailure(JobServiceContext context) {
        JobRecord record = current(context);
        if (record == null) return;
        context.unbind();
        record.execution = null;
        record.state = JobRecord.State.PENDING;
        record.failures++;
        long delay = backoff(record);
        Key key = key(record);
        dispatcher.schedule(() -> maybeStart(key, record.generation), delay, TimeUnit.MILLISECONDS);
    }

    private synchronized void maybeStart(Key key, long generation) {
        JobRecord record = jobs.get(key);
        if (record == null || record.generation != generation || record.execution != null) return;
        ConnectivitySnapshot snapshot = connectivity.snapshot();
        if (!eligible(record.job, snapshot)) {
            dispatcher.schedule(() -> maybeStart(key, generation), 1, TimeUnit.SECONDS);
            return;
        }
        Network network = record.job.getNetworkType() == JobInfo.NETWORK_TYPE_NONE
                ? null : connectivityProjection.activeNetwork(snapshot);
        JobServiceContext context = new JobServiceContext(record, bindings, processes, network, this);
        record.execution = context;
        record.state = JobRecord.State.STARTING;
        context.start();
        if (current(context) != null) {
            dispatcher.schedule(context::timeout, EXECUTION_HANDSHAKE_TIMEOUT_MS,
                    TimeUnit.MILLISECONDS);
        }
    }

    private void finish(JobRecord record, boolean reschedule) {
        JobServiceContext context = record.execution;
        if (context != null) context.unbind();
        jobs.remove(key(record), record);
        if (reschedule) {
            JobRecord retry = new JobRecord(record.uid, record.packageName, record.namespace,
                    record.job, nextGeneration());
            retry.failures = record.failures + 1;
            jobs.put(key(retry), retry);
            dispatcher.schedule(() -> maybeStart(key(retry), retry.generation), backoff(retry),
                    TimeUnit.MILLISECONDS);
        }
    }

    private void remove(Key key) {
        JobRecord record = jobs.remove(key);
        if (record != null && record.execution != null) record.execution.stop();
    }

    private JobRecord current(JobServiceContext context) {
        JobRecord current = jobs.get(key(context.record));
        return current == context.record && current.execution == context ? current : null;
    }

    private void requireOwnedJob(ApplicationProcessRegistry.AttachedApplication caller, JobInfo job) {
        if (!caller.packageName.equals(job.getService().getPackageName())) {
            throw new SecurityException("JobInfo service package does not match its caller");
        }
        ServiceInfo service = InstalledServiceInfo.service(caller.packageName,
                packages.resolveInstalledPackage(caller.packageName), job.getService().getClassName());
        if (service == null || service.applicationInfo == null || service.applicationInfo.uid != caller.uid
                || !"android.permission.BIND_JOB_SERVICE".equals(service.permission)) {
            throw new SecurityException("JobInfo service is not an owned BIND_JOB_SERVICE service");
        }
    }

    private static boolean isSupported(JobInfo job) {
        if (job == null || job.getService() == null || job.isPeriodic() || job.isPersisted()
                || job.isExpedited() || job.isUserInitiated() || job.isRequireCharging()
                || job.isRequireDeviceIdle() || job.isRequireBatteryNotLow()
                || job.isRequireStorageNotLow() || job.getTriggerContentUris() != null
                || job.getMinLatencyMillis() != 0 || job.getMaxExecutionDelayMillis() != 0) {
            return false;
        }
        int network = job.getNetworkType();
        return network == JobInfo.NETWORK_TYPE_NONE || network == JobInfo.NETWORK_TYPE_ANY
                || network == JobInfo.NETWORK_TYPE_UNMETERED;
    }

    private static boolean eligible(JobInfo job, ConnectivitySnapshot snapshot) {
        int required = job.getNetworkType();
        if (required == JobInfo.NETWORK_TYPE_NONE) return true;
        return snapshot.hasActiveNetwork()
                && (required != JobInfo.NETWORK_TYPE_UNMETERED || !snapshot.isMetered());
    }

    private static long backoff(JobRecord record) {
        long initial = Math.max(10_000L, record.job.getInitialBackoffMillis());
        if (record.job.getBackoffPolicy() == JobInfo.BACKOFF_POLICY_LINEAR) {
            return Math.min(JobInfo.MAX_BACKOFF_DELAY_MILLIS, initial * record.failures);
        }
        int shift = Math.min(20, Math.max(0, record.failures - 1));
        return Math.min(JobInfo.MAX_BACKOFF_DELAY_MILLIS, initial << shift);
    }

    private long nextGeneration() {
        if (nextGeneration == Long.MAX_VALUE) throw new IllegalStateException("generation exhausted");
        return nextGeneration++;
    }

    private static Key key(JobRecord record) {
        return new Key(record.uid, record.namespace, record.job.getId());
    }

    private static boolean equal(Object left, Object right) {
        return left == null ? right == null : left.equals(right);
    }
}
