package dev.darwinart.runtime.job;

import android.app.job.JobInfo;
import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import java.lang.reflect.Constructor;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Collections;

/**
 * Android 16 system-server boundary for the app-facing IJobScheduler contract.
 *
 * <p>Wire decoding stays separate from the Android-owned scheduler state and
 * JobService execution context.
 */
public final class JobSchedulerEndpoint extends Binder {
    // Pinned Android 16 framework.jar IJobScheduler.aidl. The generated Stub
    // assigns these methods consecutive transaction ids in declaration order.
    public static final String DESCRIPTOR = "android.app.job.IJobScheduler";
    public static final int TRANSACTION_SCHEDULE = IBinder.FIRST_CALL_TRANSACTION;
    public static final int TRANSACTION_ENQUEUE = IBinder.FIRST_CALL_TRANSACTION + 1;
    public static final int TRANSACTION_SCHEDULE_AS_PACKAGE = IBinder.FIRST_CALL_TRANSACTION + 2;
    public static final int TRANSACTION_CANCEL = IBinder.FIRST_CALL_TRANSACTION + 3;
    public static final int TRANSACTION_CANCEL_ALL = IBinder.FIRST_CALL_TRANSACTION + 4;
    public static final int TRANSACTION_CANCEL_ALL_IN_NAMESPACE = IBinder.FIRST_CALL_TRANSACTION + 5;
    public static final int TRANSACTION_GET_ALL_PENDING_JOBS = IBinder.FIRST_CALL_TRANSACTION + 6;
    public static final int TRANSACTION_GET_ALL_PENDING_JOBS_IN_NAMESPACE =
            IBinder.FIRST_CALL_TRANSACTION + 7;
    public static final int TRANSACTION_GET_PENDING_JOB = IBinder.FIRST_CALL_TRANSACTION + 8;
    public static final int TRANSACTION_GET_PENDING_JOB_REASON = IBinder.FIRST_CALL_TRANSACTION + 9;
    public static final int TRANSACTION_GET_PENDING_JOB_REASONS = IBinder.FIRST_CALL_TRANSACTION + 10;
    public static final int TRANSACTION_GET_PENDING_JOB_REASONS_HISTORY =
            IBinder.FIRST_CALL_TRANSACTION + 11;
    public static final int TRANSACTION_CAN_RUN_USER_INITIATED_JOBS =
            IBinder.FIRST_CALL_TRANSACTION + 12;
    public static final int TRANSACTION_HAS_RUN_USER_INITIATED_JOBS_PERMISSION =
            IBinder.FIRST_CALL_TRANSACTION + 13;
    public static final int TRANSACTION_GET_STARTED_JOBS = IBinder.FIRST_CALL_TRANSACTION + 14;
    public static final int TRANSACTION_GET_ALL_JOB_SNAPSHOTS = IBinder.FIRST_CALL_TRANSACTION + 15;
    public static final int TRANSACTION_REGISTER_USER_VISIBLE_JOB_OBSERVER =
            IBinder.FIRST_CALL_TRANSACTION + 16;
    public static final int TRANSACTION_UNREGISTER_USER_VISIBLE_JOB_OBSERVER =
            IBinder.FIRST_CALL_TRANSACTION + 17;
    public static final int TRANSACTION_NOTE_PENDING_USER_REQUESTED_APP_STOP =
            IBinder.FIRST_CALL_TRANSACTION + 18;

    public static final int RESULT_FAILURE = JobSchedulerService.RESULT_FAILURE;
    public static final int RESULT_SUCCESS = JobSchedulerService.RESULT_SUCCESS;
    private static final int PRIMARY_USER = 0;

    private final ApplicationProcessRegistry processes;
    private final JobSchedulerService scheduler;

    public JobSchedulerEndpoint(ApplicationProcessRegistry applicationProcesses,
            JobSchedulerService jobScheduler) {
        if (applicationProcesses == null) throw new NullPointerException("applicationProcesses");
        if (jobScheduler == null) throw new NullPointerException("jobScheduler");
        processes = applicationProcesses;
        scheduler = jobScheduler;
        attachInterface(null, DESCRIPTOR);
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == INTERFACE_TRANSACTION) {
            if (reply != null) reply.writeString(DESCRIPTOR);
            return true;
        }
        if (code < TRANSACTION_SCHEDULE || code > TRANSACTION_NOTE_PENDING_USER_REQUESTED_APP_STOP
                || reply == null) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
        }

        data.enforceInterface(DESCRIPTOR);
        switch (code) {
            case TRANSACTION_SCHEDULE:
                String scheduleNamespace = data.readString();
                JobInfo scheduled = data.readTypedObject(JobInfo.CREATOR);
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeInt(scheduler.schedule(Binder.getCallingPid(), scheduleNamespace,
                        scheduled));
                return true;
            case TRANSACTION_ENQUEUE:
                requireCaller();
                data.readString();
                JobInfo enqueueJob = data.readTypedObject(JobInfo.CREATOR);
                data.readTypedObject(android.app.job.JobWorkItem.CREATOR);
                data.enforceNoDataAvail();
                requireOwnedJob(enqueueJob, callerPackage());
                // Work-item queues are not an admitted execution mode yet.
                reply.writeNoException();
                reply.writeInt(RESULT_FAILURE);
                return true;
            case TRANSACTION_SCHEDULE_AS_PACKAGE:
                data.readString();
                data.readTypedObject(JobInfo.CREATOR);
                data.readString();
                data.readInt();
                data.readString();
                data.enforceNoDataAvail();
                throw new SecurityException("scheduleAsPackage requires UPDATE_DEVICE_STATS");
            case TRANSACTION_CANCEL:
                String cancelNamespace = data.readString();
                int cancelId = data.readInt();
                data.enforceNoDataAvail();
                scheduler.cancel(Binder.getCallingPid(), cancelNamespace, cancelId);
                reply.writeNoException();
                return true;
            case TRANSACTION_CANCEL_ALL:
                data.enforceNoDataAvail();
                scheduler.cancelAll(Binder.getCallingPid());
                reply.writeNoException();
                return true;
            case TRANSACTION_CANCEL_ALL_IN_NAMESPACE:
                String cancelAllNamespace = data.readString();
                data.enforceNoDataAvail();
                scheduler.cancelAll(Binder.getCallingPid(), cancelAllNamespace);
                reply.writeNoException();
                return true;
            case TRANSACTION_GET_ALL_PENDING_JOBS:
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeMap(asSlices(scheduler.allPending(Binder.getCallingPid())));
                return true;
            case TRANSACTION_GET_ALL_PENDING_JOBS_IN_NAMESPACE:
                String pendingNamespace = data.readString();
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeTypedObject(parceledListSlice(
                                scheduler.pending(Binder.getCallingPid(), pendingNamespace)),
                        Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
                return true;
            case TRANSACTION_GET_PENDING_JOB:
                String getNamespace = data.readString();
                int getId = data.readInt();
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeTypedObject(scheduler.pending(Binder.getCallingPid(), getNamespace,
                        getId), Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
                return true;
            case TRANSACTION_GET_PENDING_JOB_REASON:
                requireCaller();
                data.readString();
                data.readInt();
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeInt(0);
                return true;
            case TRANSACTION_GET_PENDING_JOB_REASONS:
                requireCaller();
                data.readString();
                data.readInt();
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeIntArray(new int[0]);
                return true;
            case TRANSACTION_GET_PENDING_JOB_REASONS_HISTORY:
                requireCaller();
                data.readString();
                data.readInt();
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeTypedList(Collections.emptyList());
                return true;
            case TRANSACTION_CAN_RUN_USER_INITIATED_JOBS:
                requireOwnedPackage(data.readString());
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeBoolean(false);
                return true;
            case TRANSACTION_HAS_RUN_USER_INITIATED_JOBS_PERMISSION:
                String permissionPackage = data.readString();
                int permissionUser = data.readInt();
                data.enforceNoDataAvail();
                if (permissionUser != PRIMARY_USER) {
                    throw new SecurityException("unsupported JobScheduler user " + permissionUser);
                }
                requireOwnedPackage(permissionPackage);
                reply.writeNoException();
                reply.writeBoolean(false);
                return true;
            case TRANSACTION_GET_STARTED_JOBS:
                requireCaller();
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeTypedList(Collections.emptyList());
                return true;
            case TRANSACTION_GET_ALL_JOB_SNAPSHOTS:
                requireCaller();
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeTypedObject(parceledListSlice(Collections.emptyList()),
                        Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
                return true;
            case TRANSACTION_REGISTER_USER_VISIBLE_JOB_OBSERVER:
            case TRANSACTION_UNREGISTER_USER_VISIBLE_JOB_OBSERVER:
                data.readStrongBinder();
                data.enforceNoDataAvail();
                throw new SecurityException("JobScheduler observer requires system permissions");
            case TRANSACTION_NOTE_PENDING_USER_REQUESTED_APP_STOP:
                String stopPackage = data.readString();
                int stopUser = data.readInt();
                data.readString();
                data.enforceNoDataAvail();
                if (stopUser != PRIMARY_USER) {
                    throw new SecurityException("unsupported JobScheduler user " + stopUser);
                }
                requireOwnedPackage(stopPackage);
                throw new SecurityException("JobScheduler app-stop note requires system permissions");
            default:
                return false;
        }
    }

    private String callerPackage() {
        return processes.requireIdentifiedProcess(Binder.getCallingPid());
    }

    private void requireCaller() {
        callerPackage();
    }

    private void requireOwnedPackage(String packageName) {
        String caller = callerPackage();
        if (packageName == null || !caller.equals(packageName)) {
            throw new SecurityException("JobScheduler caller does not own package " + packageName);
        }
    }

    private void requireOwnedJob(JobInfo job, String caller) {
        if (job == null || job.getService() == null) {
            throw new IllegalArgumentException("JobInfo must specify a service");
        }
        if (!caller.equals(job.getService().getPackageName())) {
            throw new SecurityException("JobScheduler caller does not own job service");
        }
    }

    private static Parcelable parceledListSlice(List<JobInfo> jobs) {
        try {
            Class<?> type = Class.forName("android.content.pm.ParceledListSlice");
            if (jobs.isEmpty()) return (Parcelable) type.getMethod("emptyList").invoke(null);
            Constructor<?> constructor = type.getDeclaredConstructor(List.class);
            constructor.setAccessible(true);
            return (Parcelable) constructor.newInstance(jobs);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    private static Map<String, Parcelable> asSlices(Map<String, List<JobInfo>> jobs) {
        HashMap<String, Parcelable> result = new HashMap<>();
        for (Map.Entry<String, List<JobInfo>> entry : jobs.entrySet()) {
            result.put(entry.getKey(), parceledListSlice(entry.getValue()));
        }
        return result;
    }
}
