package dev.darwinart.runtime.am;

import android.os.IBinder;
import java.util.HashMap;
import java.util.Map;

/** System-process ownership of attached Android application processes. */
public final class ApplicationProcessRegistry {
    private static final int FIRST_ISOLATED_UID = 99000;
    private static final int LAST_ISOLATED_UID = 99999;

    public enum InitialWork {
        ACTIVITY,
        BOUND_SERVICE
    }

    public static final class AttachedApplication {
        public final int pid;
        public final IBinder thread;
        public final String packageName;
        public final String processName;
        public final int uid;
        public final InitialWork initialWork;
        public final long startSequence;

        private AttachedApplication(
                int processId,
                IBinder applicationThread,
                String packageName,
                String processName,
                int uid,
                InitialWork initialWork,
                long sequence) {
            pid = processId;
            thread = applicationThread;
            this.packageName = packageName;
            this.processName = processName;
            this.uid = uid;
            this.initialWork = initialWork;
            startSequence = sequence;
        }
    }

    private static final class ProcessRecord {
        final long startSequence;
        final int uid;
        final InitialWork initialWork;
        IBinder thread;
        String packageName;
        String processName;
        boolean attachmentFinished;

        ProcessRecord(
                IBinder applicationThread,
                long sequence,
                String packageName,
                String processName,
                int uid,
                InitialWork initialWork) {
            thread = applicationThread;
            startSequence = sequence;
            this.packageName = packageName;
            this.processName = processName;
            this.uid = uid;
            this.initialWork = initialWork;
        }
    }

    private final HashMap<Integer, ProcessRecord> processes = new HashMap<>();

    public synchronized void beginAttachment(int pid, int uid, IBinder thread, long sequence) {
        if (pid <= 0 || uid < 0 || thread == null || sequence < 0) {
            throw new IllegalArgumentException("Invalid application attachment");
        }
        ProcessRecord reserved = processes.get(pid);
        if (reserved != null) {
            if (reserved.thread != null || reserved.startSequence != sequence
                    || reserved.uid != uid
                    || reserved.attachmentFinished) {
                throw new IllegalStateException(
                        "No matching process launch reservation: pid=" + pid
                        + " uid=" + uid + "/" + reserved.uid
                        + " seq=" + sequence + "/" + reserved.startSequence
                        + " attached=" + (reserved.thread != null)
                        + " finished=" + reserved.attachmentFinished);
            }
            reserved.thread = thread;
            return;
        }
        // The desktop launcher currently starts the package's main process
        // before AMS sees it. Service/isolated processes must be explicitly
        // reserved by the system process and never enter this fallback.
        processes.put(pid, new ProcessRecord(
                thread, sequence, null, null, uid, InitialWork.ACTIVITY));
    }

    public synchronized void reserveBoundServiceProcess(
            int pid,
            String packageName,
            String processName,
            int uid,
            long startSequence) {
        if (pid <= 0 || packageName == null || packageName.isEmpty()
                || processName == null || processName.isEmpty() || uid < 0
                || startSequence < 0) {
            throw new IllegalArgumentException("Invalid service process reservation");
        }
        if (processes.containsKey(pid)) {
            throw new IllegalStateException("Process PID is already reserved");
        }
        processes.put(pid, new ProcessRecord(
                null,
                startSequence,
                packageName,
                processName,
                uid,
                InitialWork.BOUND_SERVICE));
    }

    public synchronized void cancelBoundServiceProcess(int pid, long startSequence) {
        ProcessRecord record = require(pid);
        if (record.initialWork != InitialWork.BOUND_SERVICE
                || record.startSequence != startSequence || record.thread != null
                || record.attachmentFinished) {
            throw new IllegalStateException("No matching unactivated service process reservation");
        }
        processes.remove(pid);
    }

    public synchronized void identify(int pid, String packageName) {
        ProcessRecord record = require(pid);
        if (packageName == null || packageName.isEmpty()) {
            throw new IllegalStateException("Invalid application identity");
        }
        if (record.packageName == null) {
            record.packageName = packageName;
            record.processName = packageName;
        } else if (!record.packageName.equals(packageName)) {
            throw new SecurityException("Attached process package does not match reservation");
        }
    }

    public synchronized AttachedApplication attachmentTarget(int pid, long sequence) {
        ProcessRecord record = require(pid);
        if (record.startSequence != sequence || record.thread == null
                || record.attachmentFinished) {
            throw new IllegalStateException("No matching attachment in progress");
        }
        return snapshot(pid, record);
    }

    public synchronized void abortAttachment(int pid) {
        ProcessRecord record = processes.get(pid);
        if (record != null && !record.attachmentFinished) processes.remove(pid);
    }

    public synchronized AttachedApplication finishAttachment(int pid, long sequence) {
        ProcessRecord record = require(pid);
        if (record.startSequence != sequence || record.attachmentFinished
                || record.packageName == null || record.processName == null
                || record.thread == null) {
            throw new IllegalStateException("No matching unfinished attachment");
        }
        record.attachmentFinished = true;
        return snapshot(pid, record);
    }

    /** Removes exactly one completed process incarnation after its app-thread Binder dies. */
    public synchronized AttachedApplication retireAttached(
            int pid, long sequence, IBinder applicationThread) {
        ProcessRecord record = processes.get(pid);
        if (record == null || !record.attachmentFinished || record.startSequence != sequence
                || applicationThread == null || !applicationThread.equals(record.thread)) {
            return null;
        }
        AttachedApplication gone = snapshot(pid, record);
        processes.remove(pid);
        return gone;
    }

    public synchronized AttachedApplication requireCaller(
            int pid, IBinder suppliedThread, String callingPackage) {
        ProcessRecord record = require(pid);
        if (!record.attachmentFinished || record.packageName == null
                || suppliedThread == null || !record.thread.equals(suppliedThread)
                || !record.packageName.equals(callingPackage)) {
            throw new SecurityException("Activity caller does not match attached process");
        }
        return snapshot(pid, record);
    }

    public synchronized AttachedApplication requireAttachedProcess(int pid) {
        ProcessRecord record = require(pid);
        if (!record.attachmentFinished || record.thread == null
                || record.packageName == null || record.processName == null) {
            throw new SecurityException("Application process is not attached");
        }
        return snapshot(pid, record);
    }

    public synchronized AttachedApplication findAttached(
            String packageName, String processName, int uid) {
        for (Map.Entry<Integer, ProcessRecord> entry : processes.entrySet()) {
            ProcessRecord record = entry.getValue();
            if (record.attachmentFinished && record.thread != null
                    && packageName.equals(record.packageName)
                    && processName.equals(record.processName) && uid == record.uid) {
                return snapshot(entry.getKey(), record);
            }
        }
        return null;
    }

    /**
     * Resolves Android's isolated UID alias for an installed package.
     *
     * Isolated processes deliberately do not run as the package appId. AOSP
     * PackageManagerService resolves that kernel-owned alias before applying
     * the caller/package ownership check; callers must not treat the isolated
     * UID range as a blanket grant.
     */
    public synchronized boolean isCallerSameApp(int callerUid, String packageName) {
        if (callerUid < FIRST_ISOLATED_UID || callerUid > LAST_ISOLATED_UID
                || packageName == null || packageName.isEmpty()) {
            return false;
        }
        // Isolated UIDs are recyclable. If an old process record remains after
        // its process exited, only the newest reservation may own the UID;
        // accepting any historical match would let a recycled UID impersonate
        // its previous package.
        ProcessRecord newest = null;
        for (ProcessRecord record : processes.values()) {
            if (record.uid == callerUid
                    && (newest == null || record.startSequence > newest.startSequence)) {
                newest = record;
            }
        }
        return newest != null && packageName.equals(newest.packageName);
    }

    public synchronized String requireIdentifiedProcess(int pid) {
        ProcessRecord record = require(pid);
        if (record.packageName == null) throw new SecurityException("Unidentified application process");
        return record.packageName;
    }

    private ProcessRecord require(int pid) {
        ProcessRecord record = processes.get(pid);
        if (record == null) throw new IllegalStateException("Application process is not attached");
        return record;
    }

    private static AttachedApplication snapshot(int pid, ProcessRecord record) {
        return new AttachedApplication(
                pid,
                record.thread,
                record.packageName,
                record.processName,
                record.uid,
                record.initialWork,
                record.startSequence);
    }
}
