package dev.darwinart.runtime.am;

import android.app.IApplicationThread;
import android.app.IServiceConnection;
import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.content.res.CompatibilityInfo;
import android.os.Binder;
import android.os.DeadObjectException;
import android.os.IBinder;
import android.os.RemoteException;
import dev.darwinart.runtime.pm.PackageRecords;
import java.lang.reflect.Constructor;
import java.lang.reflect.Field;
import java.util.HashMap;

/** Behavioral coverage for owner-incarnation death and outbound Binder failures. */
public final class ActiveServicesProcessGoneTest {
    private static final String PACKAGE = "example";
    private static final String SERVICE = "example.Service";
    private static final Intent SERVICE_INTENT = new Intent()
            .setComponent(new ComponentName(PACKAGE, SERVICE));
    private static final String RECORD = "darwin-art-launch-v1\n"
            + "apk=/packages/example/base.apk\n"
            + "app_id=10042\n"
            + "metadata=apk-app-runtime: package=example application=example.App "
            + "services=example.Service>example\n";

    private static final class RecordingThread extends Binder implements IApplicationThread {
        boolean failCreate;
        boolean failBind;
        boolean failUnbind;
        boolean failStop;
        int creates;
        int binds;
        int unbinds;
        int stops;
        int exits;

        @Override public IBinder asBinder() { return this; }
        @Override public void scheduleCreateService(IBinder token, ServiceInfo info,
                CompatibilityInfo compatInfo, int processState) throws RemoteException {
            creates++;
            if (failCreate) throw new DeadObjectException();
        }
        @Override public void scheduleBindService(IBinder token, Intent intent, boolean rebind,
                int processState, long bindSeq) throws RemoteException {
            binds++;
            if (failBind) throw new RemoteException("bind failed");
        }
        @Override public void scheduleUnbindService(IBinder token, Intent intent)
                throws RemoteException {
            unbinds++;
            if (failUnbind) throw new DeadObjectException();
        }
        @Override public void scheduleStopService(IBinder token) throws RemoteException {
            stops++;
            if (failStop) throw new RemoteException("stop failed");
        }
        @Override public void scheduleExit() { exits++; }
    }

    private static final class RecordingConnection extends Binder
            implements IServiceConnection {
        boolean failConnected;
        int connected;
        @Override public IBinder asBinder() { return this; }
        @Override public void connected(ComponentName name, IBinder service, boolean dead)
                throws RemoteException {
            connected++;
            if (failConnected) throw new DeadObjectException();
        }
    }

    private static final class Harness {
        final ApplicationProcessRegistry processes = new ApplicationProcessRegistry();
        final RecordingThread owner = new RecordingThread();
        final RecordingConnection connection = new RecordingConnection();
        int launches;
        final ActiveServices active;
        final ApplicationProcessRegistry.AttachedApplication attached;

        Harness() {
            processes.beginAttachment(41, 10042, owner, 1);
            processes.identify(41, PACKAGE);
            attached = processes.finishAttachment(41, 1);
            PackageRecords.Source packages = packageName -> PACKAGE.equals(packageName)
                    ? RECORD : null;
            active = new ActiveServices(packages, processes,
                    (packageName, processName, uid, isolated, sequence) -> {
                        launches++;
                        return 88;
                    });
        }

        ServiceRecord bind() throws RemoteException {
            check(active.bindServiceInstance(41, 10042, owner, null, SERVICE_INTENT, null,
                    connection, 0, null, PACKAGE, 0) == 1);
            return onlyService(active, "servicesByToken");
        }
    }

    public static void main(String[] args) throws Exception {
        deathRetainsClientsAndRejectsStaleSnapshot();
        clientDeathRemovesBindingWithoutRestart();
        publicationFailureRemovesDeadClient();
        createAndBindFailuresBecomeDeathTransitions();
        unbindAndStopFailuresDoNotCallDeadOwner();
        unattachedServiceReservationExitsWithoutLiveWork();
        System.out.println("ActiveServices process-gone contract PASS");
    }

    private static void unattachedServiceReservationExitsWithoutLiveWork() throws Exception {
        Harness harness = new Harness();
        RecordingThread unused = new RecordingThread();
        ApplicationProcessRegistry.AttachedApplication snapshot = snapshot(
                52, unused, PACKAGE, PACKAGE + ":unused", 10042, 9);

        harness.active.onProcessAttached(snapshot);

        check(unused.exits == 1);
        check(harness.owner.exits == 0);
    }

    private static void clientDeathRemovesBindingWithoutRestart() throws Exception {
        Harness harness = new Harness();
        ServiceRecord service = harness.bind();
        IBinder.DeathRecipient death = onlyConnectionDeath(harness.active);

        death.binderDied();

        check(mapSize(harness.active, "connections") == 0);
        check(mapSize(harness.active, "connectionDeaths") == 0);
        check(onlyBinding(service).connections.isEmpty());
        check(service.retiring);
        check(harness.launches == 0);
    }

    private static void publicationFailureRemovesDeadClient() throws Exception {
        Harness harness = new Harness();
        ServiceRecord service = harness.bind();
        harness.connection.failConnected = true;

        harness.active.publishService(41, service.token, SERVICE_INTENT, new Binder());

        check(mapSize(harness.active, "connections") == 0);
        check(mapSize(harness.active, "connectionDeaths") == 0);
        check(onlyBinding(service).connections.isEmpty());
        check(service.retiring);
        check(harness.launches == 0);
    }

    private static void deathRetainsClientsAndRejectsStaleSnapshot() throws Exception {
        Harness harness = new Harness();
        ServiceRecord service = harness.bind();
        IntentBindRecord binding = onlyBinding(service);
        binding.requested = true;
        binding.bindScheduled = true;
        binding.unbindScheduled = true;
        binding.hasBound = true;
        binding.publicationReceived = true;
        binding.publishedBinder = new Binder();
        service.executingCallbacks = 7;

        harness.active.onProcessGone(harness.attached);
        check(service.applicationThread == null);
        check(service.ownerPid == 0 && service.ownerStartSequence == -1);
        check(!service.createScheduled && !service.stopScheduled);
        check(!binding.requested && !binding.bindScheduled && !binding.unbindScheduled);
        check(!binding.hasBound && !binding.publicationReceived && binding.publishedBinder == null);
        check(service.executingCallbacks == 0);
        check(binding.connections.size() == 1);
        check(harness.launches == 1);

        // A late death for the old incarnation is idempotent.
        harness.active.onProcessGone(harness.attached);
        check(harness.launches == 1);

        RecordingThread replacement = new RecordingThread();
        ApplicationProcessRegistry.AttachedApplication replacementSnapshot = snapshot(
                41, replacement, PACKAGE, PACKAGE, 10042, 2);
        harness.active.onProcessAttached(replacementSnapshot);
        check(service.applicationThread == replacement);
        check(replacement.creates == 1 && replacement.binds == 1);

        // The stale snapshot must not detach the replacement owner.
        harness.active.onProcessGone(harness.attached);
        check(service.applicationThread == replacement);
        check(harness.launches == 1);

        harness.active.onProcessGone(replacementSnapshot);
        check(service.applicationThread == null && harness.launches == 2);
        check(binding.connections.size() == 1);

        check(harness.active.unbindService(harness.connection.asBinder()));
        check(mapSize(harness.active, "servicesByToken") == 0);
    }

    private static void createAndBindFailuresBecomeDeathTransitions() throws Exception {
        Harness create = new Harness();
        create.owner.failCreate = true;
        ServiceRecord createService = create.bind();
        check(createService.applicationThread == null && createService.executingCallbacks == 0);
        check(create.launches == 1 && onlyBinding(createService).connections.size() == 1);
        create.active.onProcessGone(create.attached);
        check(create.launches == 1);
        check(create.active.unbindService(create.connection.asBinder()));

        Harness bind = new Harness();
        bind.owner.failBind = true;
        ServiceRecord bindService = bind.bind();
        check(bindService.applicationThread == null && bindService.executingCallbacks == 0);
        check(bind.launches == 1 && onlyBinding(bindService).connections.size() == 1);
        check(bind.active.unbindService(bind.connection.asBinder()));
    }

    private static void unbindAndStopFailuresDoNotCallDeadOwner() throws Exception {
        Harness unbind = new Harness();
        ServiceRecord unbindService = unbind.bind();
        unbind.owner.failUnbind = true;
        check(unbind.active.unbindService(unbind.connection.asBinder()));
        check(unbind.owner.unbinds == 1 && unbind.owner.stops == 0);
        check(mapSize(unbind.active, "servicesByToken") == 0);

        Harness stop = new Harness();
        ServiceRecord stopService = stop.bind();
        IBinder token = stopService.token;
        // Model the create/bind callbacks having completed before the final
        // unbind callback allows the retiring service to stop.
        stopService.executingCallbacks = 0;
        stop.owner.failStop = true;
        check(stop.active.unbindService(stop.connection.asBinder()));
        stop.active.unbindFinished(41, token, SERVICE_INTENT);
        check(stop.owner.stops == 1);
        check(mapSize(stop.active, "servicesByToken") == 0);
    }

    private static ApplicationProcessRegistry.AttachedApplication snapshot(int pid,
            IBinder thread, String packageName, String processName, int uid, long sequence)
            throws Exception {
        Constructor<ApplicationProcessRegistry.AttachedApplication> constructor =
                ApplicationProcessRegistry.AttachedApplication.class.getDeclaredConstructor(
                        int.class, IBinder.class, String.class, String.class, int.class,
                        ApplicationProcessRegistry.InitialWork.class, long.class);
        constructor.setAccessible(true);
        return constructor.newInstance(pid, thread, packageName, processName, uid,
                ApplicationProcessRegistry.InitialWork.BOUND_SERVICE, sequence);
    }

    @SuppressWarnings("unchecked")
    private static ServiceRecord onlyService(ActiveServices active, String field) {
        try {
            Field member = ActiveServices.class.getDeclaredField(field);
            member.setAccessible(true);
            HashMap<IBinder, ServiceRecord> values = (HashMap<IBinder, ServiceRecord>) member.get(active);
            check(values.size() == 1);
            return values.values().iterator().next();
        } catch (ReflectiveOperationException error) {
            throw new AssertionError(error);
        }
    }

    private static int mapSize(ActiveServices active, String field) {
        try {
            Field member = ActiveServices.class.getDeclaredField(field);
            member.setAccessible(true);
            return ((HashMap<?, ?>) member.get(active)).size();
        } catch (ReflectiveOperationException error) {
            throw new AssertionError(error);
        }
    }

    private static IntentBindRecord onlyBinding(ServiceRecord service) {
        check(service.bindings.size() == 1);
        return service.bindings.values().iterator().next();
    }

    @SuppressWarnings("unchecked")
    private static IBinder.DeathRecipient onlyConnectionDeath(ActiveServices active) {
        try {
            Field member = ActiveServices.class.getDeclaredField("connectionDeaths");
            member.setAccessible(true);
            HashMap<IBinder, IBinder.DeathRecipient> values =
                    (HashMap<IBinder, IBinder.DeathRecipient>) member.get(active);
            check(values.size() == 1);
            return values.values().iterator().next();
        } catch (ReflectiveOperationException error) {
            throw new AssertionError(error);
        }
    }

    private static void check(boolean value) {
        if (!value) throw new AssertionError();
    }
}
