package dev.darwinart.runtime.job;

import android.app.IServiceConnection;
import android.app.job.JobInfo;
import android.app.job.JobParameters;
import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.content.pm.ServiceInfo;
import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import dev.darwinart.runtime.am.SystemServiceBindings;
import dev.darwinart.runtime.connectivity.ConnectivitySnapshot;
import dev.darwinart.runtime.connectivity.ConnectivityState;
import dev.darwinart.runtime.am.PackageQueries;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** Behavioral host tests for the Android-owned scheduler and job callback state machine. */
public final class JobSchedulerServiceTest {
    private static final int PID = 42;
    private static final int UID = 10042;
    private static final String PACKAGE = "org.example.app";
    private static final String SERVICE = "org.example.app.JobService";
    private static final String CALLBACK_DESCRIPTOR = "android.app.job.IJobCallback";
    private static final int ACKNOWLEDGE_START = IBinder.FIRST_CALL_TRANSACTION + 2;
    private static final int JOB_FINISHED = IBinder.FIRST_CALL_TRANSACTION + 6;

    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static final class Packages implements PackageQueries {
        private final Map<String, Integer> uids = new HashMap<>();
        void add(String packageName, int uid) {
            uids.put(packageName, uid);
        }
        @Override public void addIsolatedUid(int isolatedUid, int ownerUid) {
            throw new AssertionError("job services run in their app process");
        }
        @Override public void removeIsolatedUid(int isolatedUid) {
            throw new AssertionError("job services run in their app process");
        }
        @Override public ServiceInfo service(ComponentName component, int userId) {
            Integer uid = uids.get(component.getPackageName());
            if (uid == null || !SERVICE.equals(component.getClassName())) return null;
            ApplicationInfo application = new ApplicationInfo();
            application.packageName = component.getPackageName();
            application.uid = uid;
            application.enabled = true;
            ServiceInfo info = new ServiceInfo();
            info.packageName = component.getPackageName();
            info.name = SERVICE;
            info.processName = component.getPackageName() + ":job";
            info.applicationInfo = application;
            info.enabled = true;
            info.permission = "android.permission.BIND_JOB_SERVICE";
            return info;
        }
    }

    private static final class Processes {
        final ApplicationProcessRegistry value = new ApplicationProcessRegistry();
        Processes() {
            IBinder thread = new Binder();
            value.beginAttachment(PID, UID, thread, 1L);
            value.identify(PID, 1L, thread, PACKAGE);
            value.finishAttachment(PID, 1L);
            Binder.setCallingPid(PID);
        }
    }

    private static final class Connectivity implements ConnectivityState {
        volatile ConnectivitySnapshot snapshot = ConnectivitySnapshot.unavailable();
        @Override public boolean isActiveNetworkMetered() { return snapshot.isMetered(); }
        @Override public ConnectivitySnapshot snapshot() { return snapshot; }
    }

    private static final class JobBinder extends Binder {
        int starts;
        int stops;
        JobParameters lastParameters;
        @Override protected boolean onTransact(int code, Parcel data, Parcel reply, int flags) {
            data.enforceInterface("android.app.job.IJobService");
            JobParameters parameters = data.readTypedObject((android.os.Parcelable.Creator<JobParameters>) null);
            // The test Parcel returns the typed object directly; its creator is unused.
            if (code == IBinder.FIRST_CALL_TRANSACTION) {
                starts++;
                lastParameters = parameters;
            } else if (code == IBinder.FIRST_CALL_TRANSACTION + 1) {
                stops++;
            } else {
                return false;
            }
            return true;
        }
    }

    private static final class Bindings implements SystemServiceBindings {
        int binds;
        int unbinds;
        JobBinder lastService;
        IServiceConnection lastConnection;
        boolean connect = true;

        @Override public synchronized int bindService(Intent intent, IServiceConnection connection)
                throws RemoteException {
            binds++;
            lastConnection = connection;
            if (connect) {
                lastService = new JobBinder();
                connection.connected(intent.getComponent(), lastService, false);
            }
            return 1;
        }

        @Override public synchronized boolean unbindService(IServiceConnection connection) {
            unbinds++;
            return true;
        }
    }

    private static JobInfo job(int id) {
        return new JobInfo.Builder(id, new ComponentName(PACKAGE, SERVICE)).build();
    }

    private static JobInfo networkJob(int id, int network) {
        return new JobInfo.Builder(id, new ComponentName(PACKAGE, SERVICE))
                .setRequiredNetworkType(network).build();
    }

    private static JobSchedulerService scheduler(Bindings bindings, Connectivity connectivity) {
        Packages packages = new Packages();
        packages.add(PACKAGE, UID);
        return new JobSchedulerService(packages, new Processes().value, bindings, connectivity);
    }

    private static void await(String message, Condition condition) throws Exception {
        long deadline = System.currentTimeMillis() + 2_000L;
        while (System.currentTimeMillis() < deadline) {
            if (condition.value()) return;
            Thread.sleep(10L);
        }
        throw new AssertionError(message);
    }

    private interface Condition { boolean value(); }

    private static void callback(IBinder callback, int transaction, int id, boolean value)
            throws Exception {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(CALLBACK_DESCRIPTOR);
        data.writeInt(id);
        data.writeBoolean(value);
        Parcel reply = Parcel.obtain();
        check(callback.transact(transaction, data, reply, 0), "callback transaction rejected");
        check(reply.hasNoException(), "callback omitted writeNoException");
    }

    private static void testAdmissionQueryCancelAndReplacement() throws Exception {
        Bindings bindings = new Bindings();
        Connectivity connectivity = new Connectivity();
        JobSchedulerService service = scheduler(bindings, connectivity);
        JobInfo first = job(1);
        // A network-constrained job is admitted and queryable even while no path exists.
        JobInfo constrained = networkJob(7, JobInfo.NETWORK_TYPE_ANY);
        check(service.schedule(PID, "alpha", constrained) == JobSchedulerService.RESULT_SUCCESS,
                "supported job was not admitted");
        check(service.pending(PID, "alpha", 7) == constrained, "pending lookup lost job");
        check(service.pending(PID, "alpha").size() == 1, "namespace query lost job");
        Map<String, List<JobInfo>> all = service.allPending(PID);
        check(all.get("alpha").size() == 1, "all-pending query lost namespace");
        service.cancel(PID, "alpha", 7);
        check(service.pending(PID, "alpha", 7) == null, "cancel left pending job");

        connectivity.snapshot = ConnectivitySnapshot.fromNetworkPath(true, false, false, 0);
        check(service.schedule(PID, "alpha", first) == JobSchedulerService.RESULT_SUCCESS,
                "first executable job was not admitted");
        await("first execution did not bind", () -> bindings.binds == 1);
        JobBinder oldService = bindings.lastService;
        callback(oldService.lastParameters.getCallbackForTest(), ACKNOWLEDGE_START, 1, true);
        JobInfo replacement = new JobInfo.Builder(1, new ComponentName(PACKAGE, SERVICE)).build();
        check(service.schedule(PID, "alpha", replacement) == JobSchedulerService.RESULT_SUCCESS,
                "replacement was not admitted");
        await("replacement did not bind", () -> bindings.binds == 2);
        check(oldService.stops == 1, "replacement did not stop old execution");
        check(bindings.lastService.starts == 1, "replacement start was not delivered");
        check(bindings.unbinds >= 1, "replaced execution was not unbound");
        check(service.pending(PID, "alpha", 1) == replacement,
                "replacement did not win pending lookup");
        service.cancelAll(PID, "alpha");
        check(service.pending(PID, "alpha").isEmpty(), "cancelAll(namespace) left jobs");
    }

    private static void testSupportedAndUnsupportedConstraints() throws Exception {
        Bindings bindings = new Bindings();
        Connectivity connectivity = new Connectivity();
        JobSchedulerService service = scheduler(bindings, connectivity);
        JobInfo unmetered = networkJob(2, JobInfo.NETWORK_TYPE_UNMETERED);
        check(service.schedule(PID, null, unmetered) == JobSchedulerService.RESULT_SUCCESS,
                "supported unmetered admission state was rejected");
        check(service.pending(PID, null, 2) == unmetered,
                "supported unmetered job was not queryable");
        service.cancel(PID, null, 2);
        JobInfo periodic = new JobInfo.Builder(3, new ComponentName(PACKAGE, SERVICE))
                .setPeriodic(1_000L).build();
        check(service.schedule(PID, null, periodic) == JobSchedulerService.RESULT_FAILURE,
                "periodic job was accepted");
        JobInfo charging = new JobInfo.Builder(4, new ComponentName(PACKAGE, SERVICE))
                .setRequiresCharging(true).build();
        check(service.schedule(PID, null, charging) == JobSchedulerService.RESULT_FAILURE,
                "charging constraint was accepted");
        JobInfo unsupportedNetwork = networkJob(5, 99);
        check(service.schedule(PID, null, unsupportedNetwork) == JobSchedulerService.RESULT_FAILURE,
                "unknown network constraint was accepted");
        check(bindings.binds == 0, "unsupported jobs reached execution binding");

        JobInfo waiting = networkJob(6, JobInfo.NETWORK_TYPE_ANY);
        check(service.schedule(PID, null, waiting) == JobSchedulerService.RESULT_SUCCESS,
                "supported constrained job was rejected");
        Thread.sleep(150L);
        check(bindings.binds == 0, "job ran without its required network");
        connectivity.snapshot = ConnectivitySnapshot.fromNetworkPath(true, false, false, 0);
        await("job did not run after network became available", () -> bindings.binds == 1);
        service.cancel(PID, null, 6);

        connectivity.snapshot = ConnectivitySnapshot.fromNetworkPath(true, true, false, 0);
        JobInfo unmeteredWaiting = networkJob(8, JobInfo.NETWORK_TYPE_UNMETERED);
        check(service.schedule(PID, null, unmeteredWaiting) == JobSchedulerService.RESULT_SUCCESS,
                "supported unmetered job was rejected");
        Thread.sleep(150L);
        check(bindings.binds == 1, "unmetered job ran on a metered network");
        connectivity.snapshot = ConnectivitySnapshot.fromNetworkPath(true, false, false, 0);
        await("unmetered job did not run after meteredness cleared", () -> bindings.binds == 2);
        service.cancel(PID, null, 8);
    }

    private static void testExecutionCallbackLifecycle() throws Exception {
        Bindings bindings = new Bindings();
        Connectivity connectivity = new Connectivity();
        connectivity.snapshot = ConnectivitySnapshot.fromNetworkPath(true, false, false, 0);
        JobSchedulerService service = scheduler(bindings, connectivity);
        JobInfo value = job(11);
        check(service.schedule(PID, "callbacks", value) == JobSchedulerService.RESULT_SUCCESS,
                "callback job was not admitted");
        await("callback job did not bind", () -> bindings.binds == 1);
        JobParameters parameters = bindings.lastService.lastParameters;
        check(parameters != null && parameters.getJobId() == 11,
                "start callback carried the wrong JobParameters");
        callback(parameters.getCallbackForTest(), ACKNOWLEDGE_START, 11, true);
        check(service.pending(PID, "callbacks", 11) == value,
                "acknowledged running job disappeared early");
        callback(parameters.getCallbackForTest(), JOB_FINISHED, 11, false);
        await("finished callback did not remove job", () ->
                service.pending(PID, "callbacks", 11) == null);
        check(bindings.unbinds == 1, "finished callback did not unbind execution");
    }

    private static void testOwnershipBoundary() throws Exception {
        Bindings bindings = new Bindings();
        Connectivity connectivity = new Connectivity();
        JobSchedulerService service = scheduler(bindings, connectivity);
        try {
            service.schedule(PID, null,
                    new JobInfo(new ComponentName(PACKAGE, "org.example.app.NotDeclared")));
            throw new AssertionError("undeclared service escaped ownership check");
        } catch (SecurityException expected) {}
        try {
            service.schedule(PID, null,
                    new JobInfo(new ComponentName("org.example.foreign", SERVICE)));
            throw new AssertionError("foreign package escaped ownership check");
        } catch (SecurityException expected) {}
    }

    public static void main(String[] args) throws Exception {
        testAdmissionQueryCancelAndReplacement();
        testSupportedAndUnsupportedConstraints();
        testExecutionCallbackLifecycle();
        testOwnershipBoundary();
        System.out.println("job-scheduler-service: PASS");
    }
}
