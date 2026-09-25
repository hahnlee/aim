package dev.darwinart.runtime.am;

import android.app.IApplicationThread;
import android.app.ServiceStartArgs;
import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ParceledListSlice;
import android.content.pm.ServiceInfo;
import android.content.res.CompatibilityInfo;
import android.os.Binder;
import android.os.IBinder;
import android.os.RemoteException;
import dev.darwinart.runtime.pm.PackageRecords;
import java.util.ArrayList;
import java.util.List;

/**
 * Started-service lifecycle through ActiveServices: create then start arguments,
 * stopSelf(startId) matching, stopService, foreground state and sticky restart.
 * The service runs in the caller's own process, as an app's download service does.
 */
public final class ActiveServicesStartedServiceTest {
    private static final String PACKAGE = "example";
    private static final int UID = 10042;
    private static final ComponentName COMPONENT = new ComponentName(PACKAGE, "example.Download");
    private static final Intent INTENT = new Intent().setComponent(COMPONENT);
    private static final String RECORD = "darwin-art-launch-v1\n"
            + "apk=/packages/example/base.apk\n"
            + "app_id=10042\n"
            + "metadata=apk-app-runtime: package=example application=example.App "
            + "services=example.Download>example>0>none>0>1\n";

    private static final class AppThread extends Binder implements IApplicationThread {
        final List<String> calls = new ArrayList<>();
        final List<Integer> startIds = new ArrayList<>();
        IBinder token;
        @Override public IBinder asBinder() { return this; }
        @Override public void scheduleCreateService(IBinder callbackToken, ServiceInfo info,
                CompatibilityInfo compatInfo, int processState) {
            token = callbackToken;
            calls.add("create");
        }
        @Override public void scheduleBindService(IBinder callbackToken, Intent intent,
                boolean rebind, int processState, long bindSeq) {
            calls.add("bind");
        }
        @Override public void scheduleUnbindService(IBinder callbackToken, Intent intent) {
            calls.add("unbind");
        }
        @Override public void scheduleServiceArgs(IBinder callbackToken, ParceledListSlice args) {
            calls.add("args");
            for (Object item : args.getList()) startIds.add(((ServiceStartArgs) item).startId);
        }
        @Override public void scheduleStopService(IBinder callbackToken) { calls.add("stop"); }
        @Override public void scheduleExit() { calls.add("exit"); }
    }

    private static final class NoLaunch implements ProcessLaunchTransport {
        int prepares;
        @Override public PreparedLaunch prepare(String packageName, String processName, int uid,
                boolean isolated, long startSequence) {
            prepares++;
            return new PreparedLaunch() {
                @Override public int pid() { return 90; }
                @Override public long startSequence() { return startSequence; }
            };
        }
        @Override public ActivationResult activate(PreparedLaunch prepared) {
            return ActivationResult.ACTIVE;
        }
        @Override public void retireUnattached(PreparedLaunch prepared) {}
        @Override public void onProcessGone(ApplicationProcessRegistry.AttachedApplication gone) {}
    }

    private static final class Harness {
        final ApplicationProcessRegistry processes = new ApplicationProcessRegistry();
        final AppThread app = new AppThread();
        final NoLaunch launches = new NoLaunch();
        final ActiveServices active;
        final ApplicationProcessRegistry.AttachedApplication attached;

        Harness() {
            processes.beginAttachment(41, UID, app, 1);
            processes.identify(41, 1, app, PACKAGE);
            attached = processes.finishAttachment(41, 1);
            PackageRecords.Source packages = name -> PACKAGE.equals(name) ? RECORD : null;
            active = new ActiveServices(packages, processes, launches);
        }

        ComponentName start() throws RemoteException {
            return active.startService(41, UID, app, INTENT, PACKAGE, 0);
        }

        void done(int type, int startId, int result) throws RemoteException {
            active.serviceDoneExecuting(41, UID, app.token, type, startId, result, null);
        }
    }

    public static void main(String[] args) throws Exception {
        startCreatesThenDeliversArguments();
        staleStopSelfKeepsNewerStart();
        stopServiceRetiresStartedService();
        foregroundStateFollowsServiceCalls();
        stickyServiceRestartsAfterOwnerDeath();
        notStickyServiceIsDroppedAfterOwnerDeath();
        crashLoopStopsAfterRetryLimit();
        System.out.println("ActiveServicesStartedServiceTest PASS");
    }

    private static void startCreatesThenDeliversArguments() throws Exception {
        Harness h = new Harness();
        check(COMPONENT.equals(h.start()), "startService returns the component");
        check(h.app.calls.equals(java.util.Arrays.asList("create", "args")),
                "create precedes start arguments: " + h.app.calls);
        check(h.app.startIds.equals(java.util.Arrays.asList(1)), "first start id is 1");
        h.done(0, 0, 0);
        h.done(1, 1, StartedServiceRequests.START_STICKY);
        h.start();
        check(h.app.startIds.equals(java.util.Arrays.asList(1, 2)), "second start delivers id 2");
        check(countOf(h.app.calls, "create") == 1, "a running service is not recreated");
    }

    private static void staleStopSelfKeepsNewerStart() throws Exception {
        Harness h = new Harness();
        h.start();
        h.done(0, 0, 0);
        h.done(1, 1, StartedServiceRequests.START_NOT_STICKY);
        h.start();
        h.done(1, 2, StartedServiceRequests.START_NOT_STICKY);
        check(!h.active.stopServiceToken(41, COMPONENT, h.app.token, 1),
                "stopSelf(1) must not stop a service with a newer start");
        check(!h.app.calls.contains("stop"), "no stop after stale stopSelf");
        check(h.active.stopServiceToken(41, COMPONENT, h.app.token, 2), "stopSelf(last) stops");
        check(h.app.calls.contains("stop"), "stopSelf(last) schedules onDestroy");
    }

    private static void stopServiceRetiresStartedService() throws Exception {
        Harness h = new Harness();
        check(h.active.stopService(41, UID, h.app, INTENT, 0) == 0, "not started yet");
        h.start();
        h.done(0, 0, 0);
        h.done(1, 1, StartedServiceRequests.START_STICKY);
        check(h.active.stopService(41, UID, h.app, INTENT, 0) == 1, "stopService stops");
        check(h.app.calls.contains("stop"), "stopService schedules onDestroy");
        IBinder stoppedToken = h.app.token;
        h.done(2, 0, 0);
        boolean rejected = false;
        try {
            h.active.stopServiceToken(41, COMPONENT, stoppedToken, -1);
        } catch (SecurityException expected) {
            rejected = true;
        }
        check(rejected, "a destroyed service's token is retired");
        h.start();
        check(countOf(h.app.calls, "create") == 2, "a new start recreates the service");
    }

    private static void foregroundStateFollowsServiceCalls() throws Exception {
        Harness h = new Harness();
        h.start();
        h.active.setServiceForeground(41, COMPONENT, h.app.token, 7, true, 0, 1);
        check(h.active.getForegroundServiceType(41, COMPONENT, h.app.token) == 1, "foreground type");
        h.active.setServiceForeground(41, COMPONENT, h.app.token, 0, false,
                StartedServiceRequests.STOP_FOREGROUND_REMOVE, 0);
        check(h.active.getForegroundServiceType(41, COMPONENT, h.app.token) == 0, "background");
        boolean rejected = false;
        try {
            h.active.setServiceForeground(42, COMPONENT, h.app.token, 7, true, 0, 1);
        } catch (SecurityException | IllegalStateException expected) {
            rejected = true;
        }
        check(rejected, "another process cannot drive the service's foreground state");
    }

    private static void stickyServiceRestartsAfterOwnerDeath() throws Exception {
        Harness h = new Harness();
        h.start();
        h.done(0, 0, 0);
        h.done(1, 1, StartedServiceRequests.START_STICKY);
        h.processes.retireAttached(41, 1, h.app);
        h.active.onProcessGone(h.attached);
        check(h.launches.prepares == 0, "the restart waits for the backoff");
        long deadline = System.currentTimeMillis() + 5_000;
        while (h.launches.prepares == 0 && System.currentTimeMillis() < deadline) Thread.sleep(20);
        check(h.launches.prepares == 1, "a sticky started service requests a replacement process");
    }

    private static void crashLoopStopsAfterRetryLimit() throws Exception {
        Harness h = new Harness();
        h.start();
        ServiceRecord service = serviceRecord(h.active);
        for (int crash = 0; crash < StartedServiceRequests.MAX_CRASH_RETRY; crash++) {
            // Crash while onStartCommand is executing.
            service.deliveredStarts = 1;
            StartedServiceRequests.ownerGone(service);
        }
        check(!service.startRequested, "repeated start-time crashes stop the service");
    }

    private static ServiceRecord serviceRecord(ActiveServices active) throws Exception {
        java.lang.reflect.Field field = ActiveServices.class.getDeclaredField("servicesByToken");
        field.setAccessible(true);
        java.util.Map<?, ?> services = (java.util.Map<?, ?>) field.get(active);
        return (ServiceRecord) services.values().iterator().next();
    }

    private static void notStickyServiceIsDroppedAfterOwnerDeath() throws Exception {
        Harness h = new Harness();
        h.start();
        h.done(0, 0, 0);
        h.done(1, 1, StartedServiceRequests.START_NOT_STICKY);
        h.processes.retireAttached(41, 1, h.app);
        h.active.onProcessGone(h.attached);
        check(h.launches.prepares == 0, "START_NOT_STICKY is not restarted");
    }

    private static int countOf(List<String> calls, String name) {
        int count = 0;
        for (String call : calls) if (call.equals(name)) count++;
        return count;
    }

    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }
}
