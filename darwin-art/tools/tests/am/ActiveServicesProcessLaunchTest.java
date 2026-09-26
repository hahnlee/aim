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
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

/** Focused integration coverage for ActiveServices and its phased process transport. */
public final class ActiveServicesProcessLaunchTest {
    private static final String PACKAGE = "example";
    private static final String PROCESS = PACKAGE + ":shared";
    private static final String SERVICE_A = PACKAGE + ".FirstService";
    private static final String SERVICE_B = PACKAGE + ".SecondService";
    private static final Intent INTENT_A = new Intent()
            .setComponent(new ComponentName(PACKAGE, SERVICE_A));
    private static final Intent INTENT_B = new Intent()
            .setComponent(new ComponentName(PACKAGE, SERVICE_B));
    private static final int UID = 10042;

    private interface CreateCallback {
        void run(IBinder token) throws RemoteException;
    }

    private interface BindCallback {
        void run(IBinder token, Intent intent) throws RemoteException;
    }

    private static final class RecordingThread extends Binder implements IApplicationThread {
        Throwable createFailure;
        CreateCallback createCallback;
        BindCallback bindCallback;
        boolean alive = true;
        int creates;
        int binds;

        @Override public IBinder asBinder() { return this; }
        @Override public boolean isBinderAlive() { return alive; }

        @Override public void scheduleCreateService(IBinder token, ServiceInfo info,
                CompatibilityInfo compatInfo, int processState) throws RemoteException {
            creates++;
            if (createFailure != null) {
                if (createFailure instanceof RemoteException) {
                    throw (RemoteException) createFailure;
                }
                if (createFailure instanceof RuntimeException) {
                    throw (RuntimeException) createFailure;
                }
                throw new AssertionError(createFailure);
            }
            if (createCallback != null) createCallback.run(token);
        }

        @Override public void scheduleBindService(IBinder token, Intent intent, boolean rebind,
                int processState, long bindSeq) throws RemoteException {
            binds++;
            if (bindCallback != null) bindCallback.run(token, intent);
        }

        @Override public void scheduleUnbindService(IBinder token, Intent intent)
                throws RemoteException {}
        @Override public void scheduleServiceArgs(IBinder token,
                android.content.pm.ParceledListSlice args) {
            throw new AssertionError("bound-only fixture received start arguments");
        }
        @Override public void scheduleStopService(IBinder token) throws RemoteException {}
        @Override public void scheduleExit() {}
    }

    private static final class RecordingConnection extends Binder
            implements IServiceConnection {
        int connected;

        @Override public IBinder asBinder() { return this; }
        @Override public void connected(ComponentName name, IBinder service, boolean dead)
                throws RemoteException {
            connected++;
        }
    }

    private static final class Backend implements BoundServiceProcessLauncher.Backend {
        ApplicationProcessRegistry registry;
        BoundServiceProcessLauncher launcher;
        ActiveServices active;
        Object activeMonitor;
        Runnable duringActivate;
        CountDownLatch prepareEntered;
        CountDownLatch releasePrepare;
        RuntimeException prepareFailure;
        int prepares;
        int activates;
        int cancels;
        int forgets;
        int aborts;
        long nextHandle = 71;
        int lastPid;
        long lastSequence;
        ApplicationProcessRegistry.AttachedApplication lastAttached;

        private void checkUnlocked() {
            if (activeMonitor != null && Thread.holdsLock(activeMonitor)) {
                throw new AssertionError("process backend called under ActiveServices monitor");
            }
        }

        @Override public long[] prepare(String packageName, String processName, int uid,
                boolean isolated, long startSequence) {
            checkUnlocked();
            prepares++;
            lastPid = 88;
            lastSequence = startSequence;
            if (prepareEntered != null) prepareEntered.countDown();
            if (releasePrepare != null) await(releasePrepare);
            if (prepareFailure != null) throw prepareFailure;
            return new long[] {lastPid, nextHandle++};
        }

        @Override public void activate(long handle) {
            checkUnlocked();
            activates++;
            if (duringActivate != null) duringActivate.run();
        }

        @Override public void abortPrepared(long handle) {
            checkUnlocked();
            aborts++;
        }

        @Override public void cancel(long handle) {
            checkUnlocked();
            cancels++;
        }

        @Override public void forget(long handle) {
            checkUnlocked();
            forgets++;
        }

        void attachPrepared(RecordingThread owner) {
            registry.beginAttachment(lastPid, UID, owner, lastSequence);
            registry.identify(lastPid, lastSequence, owner, PACKAGE);
            ApplicationProcessRegistry.AttachedApplication attached =
                    registry.finishAttachment(lastPid, lastSequence);
            lastAttached = attached;
            launcher.onAttached(attached);
            try {
                active.onProcessAttached(attached);
            } catch (RemoteException failure) {
                throw new AssertionError(failure);
            }
        }
    }

    private static final class Harness {
        final ApplicationProcessRegistry processes = new ApplicationProcessRegistry();
        final RecordingThread client = new RecordingThread();
        final RecordingThread owner = new RecordingThread();
        final RecordingConnection connectionA = new RecordingConnection();
        final RecordingConnection connectionB = new RecordingConnection();
        final Backend backend = new Backend();
        final BoundServiceProcessLauncher launcher;
        final ActiveServices active;

        Harness() {
            processes.beginAttachment(42, UID, client, 1);
            processes.identify(42, 1, client, PACKAGE);
            processes.finishAttachment(42, 1);
            PackageQueries packages = TestServices.resolver(PACKAGE, UID,
                    "example.FirstService", "example:shared",
                    "example.SecondService", "example:shared");
            backend.registry = processes;
            launcher = new BoundServiceProcessLauncher(processes, backend);
            active = new ActiveServices(packages, processes, launcher);
            backend.launcher = launcher;
            backend.active = active;
            backend.activeMonitor = active;
        }

        int bind(Intent intent, IBinder connection) throws RemoteException {
            return active.bindServiceInstance(42, UID, client, null, intent, null,
                    connection, 0, null, PACKAGE, 0);
        }

        ApplicationProcessRegistry.AttachedApplication attachOwner(
                int pid, long sequence, RecordingThread thread) {
            processes.reserveBoundServiceProcess(pid, PACKAGE, PROCESS, UID, sequence);
            processes.beginAttachment(pid, UID, thread, sequence);
            processes.identify(pid, sequence, thread, PACKAGE);
            return processes.finishAttachment(pid, sequence);
        }

        void attachOwnerToServices(int pid, long sequence, RecordingThread thread)
                throws RemoteException {
            active.onProcessAttached(attachOwner(pid, sequence, thread));
        }
    }

    private static void check(boolean value) {
        if (!value) throw new AssertionError("process launch integration assertion failed");
    }

    private static void await(CountDownLatch latch) {
        try {
            check(latch.await(2, TimeUnit.SECONDS));
        } catch (InterruptedException interrupted) {
            Thread.currentThread().interrupt();
            throw new AssertionError(interrupted);
        }
    }

    private static void join(Thread thread) {
        try {
            thread.join(2_000);
        } catch (InterruptedException interrupted) {
            Thread.currentThread().interrupt();
            throw new AssertionError(interrupted);
        }
        check(!thread.isAlive());
    }

    private static void blockedPrepareRetiresWithoutActivation() throws Exception {
        Harness harness = new Harness();
        harness.backend.prepareEntered = new CountDownLatch(1);
        harness.backend.releasePrepare = new CountDownLatch(1);
        AtomicReference<Throwable> bindFailure = new AtomicReference<>();
        Thread bind = new Thread(() -> {
            try {
                check(harness.bind(INTENT_A, harness.connectionA) == 1);
            } catch (Throwable failure) {
                bindFailure.set(failure);
            }
        });
        bind.start();
        await(harness.backend.prepareEntered);
        check(harness.active.unbindService((IServiceConnection) harness.connectionA));
        harness.backend.releasePrepare.countDown();
        join(bind);
        check(bindFailure.get() == null);
        check(harness.backend.prepares == 1 && harness.backend.activates == 0);
        check(harness.backend.cancels == 1 && harness.backend.forgets == 1);
    }

    private static void sharedProcessUsesOneLaunchAndRetiresAfterLastClient()
            throws Exception {
        Harness harness = new Harness();
        check(harness.bind(INTENT_A, harness.connectionA) == 1);
        check(harness.bind(INTENT_B, harness.connectionB) == 1);
        check(harness.backend.prepares == 1 && harness.backend.activates == 1);
        check(harness.active.unbindService((IServiceConnection) harness.connectionA));
        check(harness.backend.cancels == 0 && harness.backend.forgets == 0);
        check(harness.active.unbindService((IServiceConnection) harness.connectionB));
        check(harness.backend.cancels == 1 && harness.backend.forgets == 1);
    }

    private static void synchronousActivationHandsOffAndNestedBinds() throws Exception {
        Harness harness = new Harness();
        AtomicBoolean nested = new AtomicBoolean();
        harness.owner.createCallback = token -> harness.active.serviceDoneExecuting(
                88, UID, token, 0, 0, 0, null);
        harness.owner.bindCallback = (token, intent) -> {
            harness.active.publishService(88, token, intent, new Binder());
            if (nested.compareAndSet(false, true)) {
                check(harness.bind(INTENT_B, harness.connectionB) == 1);
            }
        };
        harness.backend.duringActivate = () -> harness.backend.attachPrepared(harness.owner);
        check(harness.bind(INTENT_A, harness.connectionA) == 1);
        check(nested.get());
        check(harness.backend.prepares == 1 && harness.backend.activates == 1);
        check(harness.owner.creates >= 2 && harness.owner.binds >= 2);
        check(harness.backend.cancels == 0);
        check(harness.active.unbindService((IServiceConnection) harness.connectionA));
        check(harness.active.unbindService((IServiceConnection) harness.connectionB));
        ApplicationProcessRegistry.AttachedApplication gone = harness.processes.retireAttached(
                harness.backend.lastAttached.pid, harness.backend.lastAttached.startSequence,
                harness.owner);
        check(gone != null);
        harness.active.onProcessGone(gone);
    }

    private static void prepareFailureAfterAlternateOwnerKeepsInitiator(
            boolean retireAlternateBeforeFailure) throws Exception {
        Harness harness = new Harness();
        harness.backend.prepareEntered = new CountDownLatch(1);
        harness.backend.releasePrepare = new CountDownLatch(1);
        RuntimeException failure = new IllegalStateException("old prepare failed");
        harness.backend.prepareFailure = failure;
        AtomicReference<Throwable> bindFailure = new AtomicReference<>();
        Thread bind = new Thread(() -> {
            try {
                harness.bind(INTENT_A, harness.connectionA);
            } catch (Throwable error) {
                bindFailure.set(error);
            }
        });
        bind.start();
        await(harness.backend.prepareEntered);

        RecordingThread alternate = new RecordingThread();
        ApplicationProcessRegistry.AttachedApplication alternateSnapshot =
                harness.attachOwner(89, 90, alternate);
        harness.active.onProcessAttached(alternateSnapshot);
        if (retireAlternateBeforeFailure) {
            ApplicationProcessRegistry.AttachedApplication gone =
                    harness.processes.retireAttached(89, 90, alternate);
            check(gone != null);
            harness.active.onProcessGone(gone);
        }
        harness.backend.releasePrepare.countDown();
        join(bind);
        check(bindFailure.get() == failure);
        check(harness.active.unbindService((IServiceConnection) harness.connectionA));
        check(alternate.creates == 1);
    }

    private static void ambiguousLifecycleFailureDoesNotRetireOrReplace(
            Throwable failure) throws Exception {
        Harness harness = new Harness();
        harness.owner.createFailure = failure;
        ApplicationProcessRegistry.AttachedApplication owner = harness.attachOwner(41, 1,
                harness.owner);
        harness.active.onProcessAttached(owner);
        check(harness.bind(INTENT_A, harness.connectionA) == 1);
        check(harness.owner.creates == 1);
        check(harness.backend.prepares == 0);
        check(harness.processes.hasCallerIncarnation(41, 1, harness.owner, UID));

        check(harness.bind(INTENT_A, harness.connectionB) == 1);
        check(harness.owner.creates == 1 && harness.backend.prepares == 0);

        RecordingThread replacement = new RecordingThread();
        ApplicationProcessRegistry.AttachedApplication replacementSnapshot =
                harness.attachOwner(51, 2, replacement);
        harness.active.onProcessAttached(replacementSnapshot);
        check(replacement.creates == 0);

        ApplicationProcessRegistry.AttachedApplication gone =
                harness.processes.retireAttached(41, 1, harness.owner);
        check(gone != null);
        harness.active.onProcessGone(gone);
        check(replacement.creates > 0);
        check(harness.backend.prepares == 0);
    }

    public static void main(String[] args) throws Exception {
        blockedPrepareRetiresWithoutActivation();
        sharedProcessUsesOneLaunchAndRetiresAfterLastClient();
        synchronousActivationHandsOffAndNestedBinds();
        prepareFailureAfterAlternateOwnerKeepsInitiator(false);
        prepareFailureAfterAlternateOwnerKeepsInitiator(true);
        ambiguousLifecycleFailureDoesNotRetireOrReplace(
                new RemoteException("FAILED_TRANSACTION"));
        ambiguousLifecycleFailureDoesNotRetireOrReplace(new DeadObjectException());
        System.out.println("ActiveServices process-launch integration PASS");
    }
}
