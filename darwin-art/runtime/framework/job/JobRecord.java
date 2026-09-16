package dev.darwinart.runtime.job;

import android.app.job.JobInfo;

/** System-server state for one admitted Android job. */
final class JobRecord {
    enum State { PENDING, BINDING, STARTING, RUNNING }

    final int uid;
    final String packageName;
    final String namespace;
    final JobInfo job;
    final long generation;
    int failures;
    State state = State.PENDING;
    JobServiceContext execution;

    JobRecord(int ownerUid, String ownerPackage, String jobNamespace, JobInfo jobInfo,
            long sequence) {
        uid = ownerUid;
        packageName = ownerPackage;
        namespace = jobNamespace;
        job = jobInfo;
        generation = sequence;
    }
}
