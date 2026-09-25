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
import dev.darwinart.runtime.pm.ServiceResolver;
import java.lang.reflect.Constructor;
import java.lang.reflect.Field;
import java.util.HashMap;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

/** Behavioral coverage for owner-incarnation death and outbound Binder failures. */
public final class ActiveServicesProcessGoneTest {
    private static final String PACKAGE = "example";
    private static final String SERVICE = "example.Service";
    private static final Intent SERVICE_INTENT = new Intent()
            .setComponent(new ComponentName(PACKAGE, SERVICE));

    /** Controlled phased transport used only to exercise ActiveServices policy. */
    private static final class LaunchTransport implements ProcessLaunchTransport {
        private enum Phase { PREPARED, ACTIVE, RETIRED }

        private static final class Token implements PreparedLaunch {
            final LaunchTransport owner;
            final long sequence;
            Phase phase = Phase.PREPARED;

            Token(LaunchTransport transport, long startSequence) {
                owner = transport;
                sequence = startSequence;
            }

            @Override public int pid() { return 88; }
            @Override public long startSequence() { return sequence; }
        }

        final Runnable onActivation;
        Object activeMonitor;
        int prepares;
        int activations;
        int retirements;

        LaunchTransport(Runnable activationObserver) {
            onActivation = activationObserver;
        }

        private void checkUnlocked() {
            if (activeMonitor != null && Thread.holdsLock(activeMonitor)) {
                throw new AssertionError("process launch transport called under ActiveServices lock");
            }
        }

        private Token requireToken(PreparedLaunch prepared) {
            if (!(prepared instanceof Token) || ((Token) prepared).owner != this) {
                throw new IllegalArgumentException("foreign process launch token");
            }
            return (Token) prepared;
        }

        @Override public PreparedLaunch prepare(String packageName, String processName, int uid,
                boolean isolated, long startSequence) {
            checkUnlocked();
            Token token = new Token(this, startSequence);
            synchronized (this) {
                prepares++;
            }
            return token;
        }

        @Override public ActivationResult activate(PreparedLaunch prepared) {
            checkUnlocked();
            Token token = requireToken(prepared);
            synchronized (this) {
                if (token.phase == Phase.RETIRED) return ActivationResult.CANCELLED;
                if (token.phase != Phase.PREPARED) {
                    throw new IllegalStateException("duplicate process launch activation");
                }
                token.phase = Phase.ACTIVE;
                activations++;
            }
            onActivation.run();
            return ActivationResult.ACTIVE;
        }

        @Override public void retireUnattached(PreparedLaunch prepared) {
            checkUnlocked();
            Token token = requireToken(prepared);
            synchronized (this) {
                if (token.phase != Phase.RETIRED) {
                    token.phase = Phase.RETIRED;
                    retirements++;
                }
            }
        }

        @Override public void onProcessGone(ApplicationProcessRegistry.AttachedApplication gone) {
            checkUnlocked();
            // Manual owner PIDs 41/51 are independent of this fixture's prepared PID 88.
            if (gone != null && gone.pid == 88) {
                throw new AssertionError("Prepared fixture actor needs an exact token death transition");
            }
        }
    }

    private interface CreateCallback {
        void run(IBinder token) throws RemoteException;
    }

    private interface BindCallback {
        void run(IBinder token, Intent intent) throws RemoteException;
    }

    private interface UnbindCallback {
        void run(IBinder token, Intent intent) throws RemoteException;
    }

    private interface StopCallback {
        void run(IBinder token) throws RemoteException;
    }

    private static final class RecordingThread extends Binder implements IApplicationThread {
        boolean failCreate;
        boolean failBind;
        boolean failUnbind;
        boolean failStop;
        boolean alive = true;
        CreateCallback createCallback;
        BindCallback bindCallback;
        UnbindCallback unbindCallback;
        StopCallback stopCallback;
        Runnable callbackProbe;
        int creates;
        int binds;
        int rebindBinds;
        int unbinds;
        int stops;
        int exits;

        @Override public IBinder asBinder() { return this; }
        @Override public boolean isBinderAlive() { return alive; }
        @Override public void scheduleCreateService(IBinder token, ServiceInfo info,
                CompatibilityInfo compatInfo, int processState) throws RemoteException {
            if (callbackProbe != null) callbackProbe.run();
            creates++;
            if (failCreate) {
                alive = false;
                throw new DeadObjectException();
            }
            if (createCallback != null) createCallback.run(token);
        }
        @Override public void scheduleBindService(IBinder token, Intent intent, boolean rebind,
                int processState, long bindSeq) throws RemoteException {
            if (callbackProbe != null) callbackProbe.run();
            binds++;
            if (rebind) rebindBinds++;
            if (failBind) {
                alive = false;
                throw new DeadObjectException();
            }
            if (bindCallback != null) bindCallback.run(token, intent);
        }
        @Override public void scheduleUnbindService(IBinder token, Intent intent)
                throws RemoteException {
            if (callbackProbe != null) callbackProbe.run();
            unbinds++;
            if (failUnbind) {
                alive = false;
                throw new DeadObjectException();
            }
            if (unbindCallback != null) unbindCallback.run(token, intent);
        }
        @Override public void scheduleServiceArgs(IBinder token,
                android.content.pm.ParceledListSlice args) {
            throw new AssertionError("bound-only fixture received start arguments");
        }
        @Override public void scheduleStopService(IBinder token) throws RemoteException {
            if (callbackProbe != null) callbackProbe.run();
            stops++;
            if (failStop) {
                alive = false;
                throw new DeadObjectException();
            }
            if (stopCallback != null) stopCallback.run(token);
        }
        @Override public void scheduleExit() { exits++; }
    }

    private static final class RecordingConnection extends Binder
            implements IServiceConnection {
        boolean failConnected;
        boolean failLink;
        RuntimeException linkFailure;
        RuntimeException unlinkFailureOnce;
        boolean dieBeforeInstall;
        Runnable beforeLink;
        Runnable beforeUnlink;
        CreateCallback connectedCallback;
        int links;
        int unlinks;
        int connected;
        @Override public IBinder asBinder() { return this; }
        @Override public void linkToDeath(IBinder.DeathRecipient recipient, int flags)
                throws RemoteException {
            links++;
            Runnable callback = beforeLink;
            beforeLink = null;
            if (callback != null) callback.run();
            if (dieBeforeInstall) recipient.binderDied();
            if (linkFailure != null) throw linkFailure;
            if (failLink) throw new RemoteException("link failed");
            super.linkToDeath(recipient, flags);
        }
        @Override public boolean unlinkToDeath(IBinder.DeathRecipient recipient, int flags) {
            unlinks++;
            Runnable callback = beforeUnlink;
            beforeUnlink = null;
            if (callback != null) callback.run();
            RuntimeException failure = unlinkFailureOnce;
            unlinkFailureOnce = null;
            if (failure != null) throw failure;
            return super.unlinkToDeath(recipient, flags);
        }
        @Override public void connected(ComponentName name, IBinder service, boolean dead)
                throws RemoteException {
            connected++;
            if (connectedCallback != null) connectedCallback.run(service);
            if (failConnected) throw new DeadObjectException();
        }
    }

    private static final class Harness {
        final ApplicationProcessRegistry processes = new ApplicationProcessRegistry();
        final RecordingThread owner = new RecordingThread();
        final RecordingThread client = new RecordingThread();
        final RecordingConnection connection = new RecordingConnection();
        final LaunchTransport transport;
        int launches;
        final ActiveServices active;
        final ApplicationProcessRegistry.AttachedApplication attached;
        final ApplicationProcessRegistry.AttachedApplication clientAttached;

        Harness() {
            // The caller process and service process are separate Android
            // owners.  The service's reserved process name must match the
            // metadata so findAttached cannot accidentally reuse the client.
            processes.beginAttachment(42, 10042, client, 2);
            processes.identify(42, 2, client, PACKAGE);
            clientAttached = processes.finishAttachment(42, 2);
            processes.reserveBoundServiceProcess(41, PACKAGE, PACKAGE + ":client", 10042, 1);
            processes.beginAttachment(41, 10042, owner, 1);
            processes.identify(41, 1, owner, PACKAGE);
            attached = processes.finishAttachment(41, 1);
            ServiceResolver packages = TestServices.resolver(PACKAGE, 10042,
                    "example.Service", "example:client");
            transport = new LaunchTransport(() -> launches++);
            active = new ActiveServices(packages, processes, transport);
            transport.activeMonitor = active;
        }

        ServiceRecord bind() throws RemoteException {
            check(active.bindServiceInstance(42, 10042, client, null, SERVICE_INTENT, null,
                    connection, 0, null, PACKAGE, 0) == 1);
            return onlyService(active, "servicesByToken");
        }

        boolean activeOwnerIs(RecordingThread expected) {
            ServiceRecord service = onlyService(active, "servicesByToken");
            return service.applicationThread != null
                    && service.applicationThread.asBinder().equals(expected);
        }

        ApplicationProcessRegistry.AttachedApplication attachBoundServiceOwner(
                int pid, RecordingThread thread, long sequence) {
            processes.reserveBoundServiceProcess(pid, PACKAGE, PACKAGE + ":client", 10042,
                    sequence);
            processes.beginAttachment(pid, 10042, thread, sequence);
            processes.identify(pid, sequence, thread, PACKAGE);
            return processes.finishAttachment(pid, sequence);
        }
    }

    public static void main(String[] args) throws Exception {
        if (args.length == 1 && "--unlocked-notification-boundary".equals(args[0])) {
            notificationReentryMustNotRetainActiveServicesMonitor();
            return;
        }
        deathRetainsClientsAndRejectsStaleSnapshot();
        clientDeathRemovesBindingWithoutRestart();
        publicationFailureRemovesDeadClient();
        createAndBindFailuresBecomeDeathTransitions();
        unbindAndStopFailuresDoNotCallDeadOwner();
        unusedServiceAttachmentDoesNotQuitMainLooper();
        linkReentryRetiresPendingClientBeforeSuccessfulLink();
        blockedLinkRetirementFinishesBeforeLinkRelease();
        runtimeLinkFailurePropagatesWithoutDeath();
        synchronousConnectionDeathClosesLateSuccessfulLink();
        linkErrorPreservesFailedResourceTail();
        failedLinkPreservesPendingUnbindToken();
        synchronousLifecycleCompletionsSettleExactlyOnce();
        synchronousBatchUnbindRetainsOnePendingStop();
        stalePublicationCannotCompleteReplacementCallbacks();
        staleOwnerDispatchCannotClearReplacement();
        callbackTransportsRunOutsideActiveServicesMonitor();
        crossThreadProgressDuringBlockedDispatch();
        rebindWaitsBehindBlockedFinalClientUnbind();
        latchedUnbindSurvivesNewDemandAndRebinds();
        falseUnbindCompletionSettlesExactlyOnce();
        uncheckedCreateFailureSealsOldOwnerUntilDeath();
        initialAndCachedNotificationsAreUnlockedAndDeduplicated();
        admittedNotificationDetachedBeforeClaimIsCancelled();
        detachedNotificationCannotDeleteReplacementRegistration();
        pendingNotificationDetachedBeforeClaimIsCancelled();
        nullPublicationNotifiesOnceAndSettlesBindSeparately();
        replacementPublicationWaitsForOldNotificationTail();
        newOwnerProgressesBeforeOldOwnerTailCompletes();
        staleSameUidCompletionCannotTouchNewOwner();
        callbackCapabilityAdmissionContract();
        System.out.println("ActiveServices process-gone contract PASS");
    }

    /** Guest callback tokens are per-owner capabilities, including PID-zero callbacks. */
    private static void callbackCapabilityAdmissionContract() throws Exception {
        Harness harness = new Harness();
        ServiceRecord service = harness.bind();
        IBinder oldToken = service.token;

        // PID zero is admitted only through the capability's authenticated UID and
        // exact live registry incarnation. Duplicate completion remains idempotent.
        harness.active.serviceDoneExecuting(0, 10042, oldToken, 0, 0, 0, null);
        harness.active.serviceDoneExecuting(0, 10042, oldToken, 0, 0, 0, null);
        // The bind callback is still pending; CREATE was settled exactly once.
        check(service.executingCallbacks == 1);
        try {
            harness.active.serviceDoneExecuting(0, 10043, oldToken, 0, 0, 0, null);
            throw new AssertionError("wrong UID callback was accepted");
        } catch (SecurityException expected) { }
        try {
            harness.active.serviceDoneExecuting(0, 10042, new Binder(), 0, 0, 0, null);
            throw new AssertionError("unknown callback capability was accepted");
        } catch (SecurityException expected) { }

        ApplicationProcessRegistry.AttachedApplication gone = harness.processes.retireAttached(
                41, 1, harness.owner);
        check(gone != null);
        harness.active.onProcessGone(gone);
        try {
            harness.active.serviceDoneExecuting(0, 10042, oldToken, 0, 0, 0, null);
            throw new AssertionError("retired callback capability was accepted");
        } catch (SecurityException expected) { }

        RecordingThread replacement = new RecordingThread();
        replacement.createCallback = token -> harness.active.serviceDoneExecuting(
                0, 10042, token, 0, 0, 0, null);
        replacement.bindCallback = (token, intent) -> harness.active.publishService(
                41, token, intent, new Binder());
        replacement.unbindCallback = (token, intent) -> harness.active.serviceDoneExecuting(
                0, 10042, token, 4, 0, 0, intent);
        replacement.stopCallback = token -> harness.active.serviceDoneExecuting(
                0, 10042, token, 2, 0, 0, null);
        ApplicationProcessRegistry.AttachedApplication snapshot = harness.attachBoundServiceOwner(
                41, replacement, 2);
        harness.active.onProcessAttached(snapshot);
        IBinder newToken = service.token;
        check(newToken != oldToken);
        try {
            harness.active.serviceDoneExecuting(0, 10042, oldToken, 0, 0, 0, null);
            throw new AssertionError("old owner capability reached replacement");
        } catch (SecurityException expected) { }

        // Synchronous callbacks still require the positive, exact owner PID.
        try {
            harness.active.publishService(0, newToken, SERVICE_INTENT, new Binder());
            throw new AssertionError("PID-zero publication was accepted");
        } catch (SecurityException expected) { }
        check(harness.active.unbindService(harness.connection.asBinder()));
        check(mapSize(harness.active, "servicesByToken") == 0);
    }

    private static void callbackTransportsRunOutsideActiveServicesMonitor() throws Exception {
        Harness harness = new Harness();
        harness.owner.callbackProbe = () -> check(!Thread.holdsLock(harness.active));
        harness.owner.createCallback = token -> harness.active.serviceDoneExecuting(
                41, 10042, token, 0, 0, 0, null);
        harness.owner.bindCallback = (token, intent) -> harness.active.publishService(
                41, token, intent, new Binder());
        harness.owner.unbindCallback = (token, intent) -> harness.active.serviceDoneExecuting(
                41, 10042, token, 4, 0, 0, intent);
        harness.owner.stopCallback = token -> harness.active.serviceDoneExecuting(
                41, 10042, token, 2, 0, 0, null);
        harness.bind();
        check(harness.active.unbindService(harness.connection.asBinder()));
        check(harness.owner.creates == 1 && harness.owner.binds == 1);
        check(harness.owner.unbinds == 1 && harness.owner.stops == 1);
    }

    private static void crossThreadProgressDuringBlockedDispatch() throws Exception {
        Harness harness = new Harness();
        CountDownLatch entered = new CountDownLatch(1);
        CountDownLatch release = new CountDownLatch(1);
        AtomicBoolean blockOnce = new AtomicBoolean(true);
        AtomicReference<Throwable> bindFailure = new AtomicReference<>();
        harness.owner.callbackProbe = () -> {
            if (!blockOnce.compareAndSet(true, false)) return;
            entered.countDown();
            try {
                check(release.await(2, TimeUnit.SECONDS));
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw new AssertionError(error);
            }
        };
        Thread binder = new Thread(() -> {
            try {
                harness.bind();
            } catch (Throwable error) {
                bindFailure.set(error);
            }
        });
        binder.start();
        check(entered.await(2, TimeUnit.SECONDS));
        CountDownLatch progressed = new CountDownLatch(1);
        Thread completion = new Thread(() -> {
            ApplicationProcessRegistry.AttachedApplication gone = harness.processes.retireAttached(
                    harness.attached.pid, harness.attached.startSequence, harness.owner);
            check(gone != null);
            harness.active.onProcessGone(gone);
            progressed.countDown();
        });
        completion.start();
        boolean madeProgress = progressed.await(250, TimeUnit.MILLISECONDS);
        release.countDown();
        binder.join(2_000);
        completion.join(2_000);
        check(!binder.isAlive() && !completion.isAlive());
        check(bindFailure.get() == null);
        check(madeProgress);
    }

    private static void rebindWaitsBehindBlockedFinalClientUnbind() throws Exception {
        Harness harness = new Harness();
        ServiceRecord service = harness.bind();
        harness.active.serviceDoneExecuting(41, 10042, service.token, 0, 0, 0, null);
        harness.active.publishService(41, service.token, SERVICE_INTENT, new Binder());
        CountDownLatch unbindEntered = new CountDownLatch(1);
        CountDownLatch unbindRelease = new CountDownLatch(1);
        harness.owner.unbindCallback = (token, intent) -> {
            unbindEntered.countDown();
            try {
                check(unbindRelease.await(2, TimeUnit.SECONDS));
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw new AssertionError(error);
            }
            harness.active.unbindFinished(41, token, intent);
        };
        Thread unbinder = new Thread(() -> {
            try {
                check(harness.active.unbindService(harness.connection.asBinder()));
            } catch (RemoteException error) {
                throw new AssertionError(error);
            }
        });
        unbinder.start();
        check(unbindEntered.await(2, TimeUnit.SECONDS));
        check(onlyBinding(service).unbindScheduled);
        RecordingConnection replacement = new RecordingConnection();
        check(harness.active.bindServiceInstance(42, 10042, harness.client, null,
                SERVICE_INTENT, null, replacement, 0, null, PACKAGE, 0) == 1);
        unbindRelease.countDown();
        unbinder.join(2_000);
        check(!unbinder.isAlive());
        check(connectionRecords(harness.active, replacement).size() == 1);
        check(harness.owner.unbinds == 1);
        check(harness.owner.binds >= 2);
        check(harness.owner.rebindBinds >= 1);
        check(service.bindings.size() == 1);
    }

    private static void latchedUnbindSurvivesNewDemandAndRebinds() throws Exception {
        Harness harness = new Harness();
        harness.owner.callbackProbe = () -> check(!Thread.holdsLock(harness.active));
        harness.owner.createCallback = token -> harness.active.serviceDoneExecuting(
                41, 10042, token, 0, 0, 0, null);
        CountDownLatch bindEntered = new CountDownLatch(1);
        CountDownLatch releaseBind = new CountDownLatch(1);
        harness.owner.bindCallback = (token, intent) -> {
            if (harness.owner.rebindBinds != 0) {
                harness.active.serviceDoneExecuting(41, 10042, token, 3, 0, 0, intent);
            } else {
                bindEntered.countDown();
                try {
                    check(releaseBind.await(2, TimeUnit.SECONDS));
                } catch (InterruptedException error) {
                    Thread.currentThread().interrupt();
                    throw new AssertionError(error);
                }
                harness.active.publishService(41, token, intent, new Binder());
            }
        };
        harness.owner.unbindCallback = (token, intent) -> {
            check(!Thread.holdsLock(harness.active));
            // The legacy unbindFinished callback means onUnbind returned true;
            // the next bind must therefore be the rebind=true edge.
            harness.active.unbindFinished(41, token, intent);
        };
        AtomicReference<Throwable> bindFailure = new AtomicReference<>();
        Thread binder = new Thread(() -> {
            try {
                harness.bind();
            } catch (Throwable error) {
                bindFailure.set(error);
            }
        });
        binder.start();
        check(bindEntered.await(2, TimeUnit.SECONDS));
        ServiceRecord service = onlyService(harness.active, "servicesByToken");
        IntentBindRecord binding = onlyBinding(service);
        check(harness.active.unbindService(harness.connection.asBinder()));
        check(binding.unbindRequested && !binding.unbindScheduled);
        check(harness.owner.unbinds == 0);

        RecordingConnection replacement = new RecordingConnection();
        CountDownLatch demandFinished = new CountDownLatch(1);
        AtomicReference<Throwable> demandFailure = new AtomicReference<>();
        Thread demander = new Thread(() -> {
            try {
                check(harness.active.bindServiceInstance(42, 10042, harness.client, null,
                        SERVICE_INTENT, null, replacement, 0, null, PACKAGE, 0) == 1);
            } catch (Throwable error) {
                demandFailure.set(error);
            } finally {
                demandFinished.countDown();
            }
        });
        demander.start();
        check(demandFinished.await(500, TimeUnit.MILLISECONDS));
        check(demandFailure.get() == null);
        // Demand returns before UNBIND is even claimed, not merely before its tail.
        check(releaseBind.getCount() == 1);
        check(binding.unbindRequested && !binding.unbindScheduled);
        check(harness.owner.unbinds == 0 && harness.owner.rebindBinds == 0);

        releaseBind.countDown();
        binder.join(2_000);
        demander.join(2_000);
        check(!binder.isAlive() && !demander.isAlive());
        check(bindFailure.get() == null);
        check(harness.owner.unbinds == 1);
        check(harness.owner.binds == 2 && harness.owner.rebindBinds == 1);
        check(service.executingCallbacks == 0);
        check(!binding.unbindScheduled && !binding.rebindCallbackPending);
        check(binding.hasBound && binding.connections.size() == 1);
    }

    private static void falseUnbindCompletionSettlesExactlyOnce() throws Exception {
        Harness harness = new Harness();
        harness.owner.callbackProbe = () -> check(!Thread.holdsLock(harness.active));
        harness.owner.createCallback = token -> harness.active.serviceDoneExecuting(
                41, 10042, token, 0, 0, 0, null);
        harness.owner.bindCallback = (token, intent) -> harness.active.publishService(
                41, token, intent, new Binder());
        int[] callbackCounts = {-1, -1};
        harness.owner.unbindCallback = (token, intent) -> {
            check(new Intent.FilterComparison(SERVICE_INTENT).equals(
                    new Intent.FilterComparison(intent)));
            harness.active.serviceDoneExecuting(41, 10042, token, 4, 0, 0, intent);
            // A duplicate type-4 completion must not decrement twice.
            harness.active.serviceDoneExecuting(41, 10042, token, 4, 0, 0, intent);
            callbackCounts[0] = onlyService(harness.active, "servicesByToken").executingCallbacks;
        };
        harness.owner.stopCallback = token -> {
            harness.active.serviceDoneExecuting(41, 10042, token, 2, 0, 0, null);
            // Nor may a duplicate STOP completion underflow the reservation.
            harness.active.serviceDoneExecuting(41, 10042, token, 2, 0, 0, null);
            callbackCounts[1] = onlyService(harness.active, "servicesByToken").executingCallbacks;
        };
        ServiceRecord service = harness.bind();
        IntentBindRecord binding = onlyBinding(service);
        check(service.executingCallbacks == 0 && binding.hasBound);
        check(harness.active.unbindService(harness.connection.asBinder()));
        check(callbackCounts[0] == 0 && callbackCounts[1] == 0);
        check(harness.owner.creates == 1 && harness.owner.binds == 1);
        check(harness.owner.unbinds == 1 && harness.owner.stops == 1);
        check(service.executingCallbacks == 0);
        check(mapSize(harness.active, "servicesByToken") == 0);
    }

    private static void uncheckedCreateFailureSealsOldOwnerUntilDeath() throws Exception {
        Harness harness = new Harness();
        AtomicReference<Throwable> failure = new AtomicReference<>();
        IllegalStateException original = new IllegalStateException("unchecked create transport failure");
        harness.owner.createCallback = token -> {
            throw original;
        };
        harness.owner.bindCallback = (token, intent) -> {
            throw new AssertionError("BIND dispatched after unchecked CREATE failure");
        };

        try {
            harness.bind();
        } catch (Throwable error) {
            failure.set(error);
        }
        check(failure.get() == original);
        ServiceRecord service = onlyService(harness.active, "servicesByToken");
        IntentBindRecord binding = onlyBinding(service);
        check(harness.owner.creates == 1 && harness.owner.binds == 0);
        check(service.createScheduled && service.createCallbackPending);
        check(service.executingCallbacks == 1);

        // Re-entering with the same attached incarnation must not retry an
        // operation whose enqueue outcome is ambiguous.
        harness.active.onProcessAttached(harness.attached);
        check(harness.owner.creates == 1 && harness.owner.binds == 0);
        check(service.executingCallbacks == 1 && binding.bindScheduled == false);

        ApplicationProcessRegistry.AttachedApplication old = harness.processes.retireAttached(
                harness.attached.pid, harness.attached.startSequence, harness.owner);
        check(old != null);
        harness.active.onProcessGone(old);
        harness.active.onProcessAttached(harness.attached);
        check(service.applicationThread == null && service.lifecycleLane == null);
        check(harness.owner.creates == 1 && harness.owner.binds == 0);

        RecordingThread replacement = new RecordingThread();
        replacement.createCallback = token -> harness.active.serviceDoneExecuting(
                51, 10042, token, 0, 0, 0, null);
        replacement.bindCallback = (token, intent) -> harness.active.publishService(
                51, token, intent, new Binder());
        ApplicationProcessRegistry.AttachedApplication replacementSnapshot =
                harness.attachBoundServiceOwner(51, replacement, 2);
        harness.active.onProcessAttached(replacementSnapshot);
        check(replacement.creates == 1 && replacement.binds == 1);
        check(service.applicationThread == replacement);
        check(service.executingCallbacks == 0);

        // The retired snapshot is no longer an authenticated registry
        // incarnation and cannot reopen the sealed old lane.
        harness.active.onProcessAttached(harness.attached);
        check(service.applicationThread == replacement);
        check(replacement.creates == 1 && replacement.binds == 1);
    }

    private static void newOwnerProgressesBeforeOldOwnerTailCompletes() throws Exception {
        Harness harness = new Harness();
        CountDownLatch oldEntered = new CountDownLatch(1);
        CountDownLatch oldRelease = new CountDownLatch(1);
        CountDownLatch newCreate = new CountDownLatch(1);
        CountDownLatch newBind = new CountDownLatch(1);
        AtomicReference<Throwable> oldFailure = new AtomicReference<>();
        harness.owner.createCallback = token -> {
            oldEntered.countDown();
            try {
                check(oldRelease.await(2, TimeUnit.SECONDS));
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw new AssertionError(error);
            }
            throw new RemoteException("old owner transport failed after replacement");
        };
        Thread oldDispatch = new Thread(() -> {
            try {
                harness.bind();
            } catch (Throwable error) {
                oldFailure.set(error);
            }
        });
        oldDispatch.start();
        check(oldEntered.await(2, TimeUnit.SECONDS));

        ApplicationProcessRegistry.AttachedApplication old = harness.processes.retireAttached(
                harness.attached.pid, harness.attached.startSequence, harness.owner);
        check(old != null);
        harness.active.onProcessGone(old);
        RecordingThread replacement = new RecordingThread();
        replacement.createCallback = token -> newCreate.countDown();
        replacement.bindCallback = (token, intent) -> newBind.countDown();
        ApplicationProcessRegistry.AttachedApplication replacementSnapshot =
                harness.attachBoundServiceOwner(51, replacement, 2);
        Thread newAttach = new Thread(() -> {
            try {
                harness.active.onProcessAttached(replacementSnapshot);
            } catch (RemoteException error) {
                throw new AssertionError(error);
            }
        });
        newAttach.start();
        check(newCreate.await(500, TimeUnit.MILLISECONDS));
        check(newBind.await(500, TimeUnit.MILLISECONDS));
        check(replacement.creates == 1 && replacement.binds == 1);
        check(harness.activeOwnerIs(replacement));

        oldRelease.countDown();
        oldDispatch.join(2_000);
        newAttach.join(2_000);
        check(!oldDispatch.isAlive() && !newAttach.isAlive());
        check(oldFailure.get() == null);
        check(harness.owner.creates == 1);
        check(replacement.creates == 1 && replacement.binds == 1);
    }

    private static void staleSameUidCompletionCannotTouchNewOwner() throws Exception {
        Harness harness = new Harness();
        ServiceRecord service = harness.bind();
        ApplicationProcessRegistry.AttachedApplication gone = harness.processes.retireAttached(
                harness.attached.pid, harness.attached.startSequence, harness.owner);
        check(gone != null);
        harness.active.onProcessGone(gone);
        RecordingThread reusedPid = new RecordingThread();
        harness.processes.beginAttachment(41, 10042, reusedPid, 3);
        harness.processes.identify(41, 3, reusedPid, PACKAGE);
        harness.processes.finishAttachment(41, 3);

        RecordingThread replacement = new RecordingThread();
        ApplicationProcessRegistry.AttachedApplication replacementSnapshot =
                harness.attachBoundServiceOwner(51, replacement, 2);
        harness.active.onProcessAttached(replacementSnapshot);
        check(service.applicationThread == replacement);
        check(service.ownerPid == 51 && service.ownerStartSequence == 2);
        check(service.executingCallbacks == 2);
        try {
            harness.active.serviceDoneExecuting(41, 10042, service.token, 0, 0, 0, null);
            throw new AssertionError("stale same-UID completion was accepted");
        } catch (SecurityException expected) {
            // The old PID is reused by another identified incarnation, proving
            // owner-incarnation matching rather than a missing-caller reject.
        }
        check(service.applicationThread == replacement);
        check(service.ownerPid == 51 && service.ownerStartSequence == 2);
        check(service.executingCallbacks == 2);
    }

    private static void unusedServiceAttachmentDoesNotQuitMainLooper() throws Exception {
        Harness harness = new Harness();
        RecordingThread unused = new RecordingThread();
        ApplicationProcessRegistry.AttachedApplication snapshot = snapshot(
                52, unused, PACKAGE, PACKAGE + ":unused", 10042, 9);

        harness.active.onProcessAttached(snapshot);

        check(unused.exits == 0);
        check(unused.creates == 0 && unused.binds == 0 && unused.stops == 0);
        check(harness.owner.exits == 0);
    }

    private static void clientDeathRemovesBindingWithoutRestart() throws Exception {
        Harness harness = new Harness();
        ServiceRecord service = harness.bind();
        // Register the same client twice and a separate system-owned
        // connection.  Client death must detach all exact client records but
        // leave the live system owner indexed for the service restart.
        check(harness.active.bindServiceInstance(42, 10042, harness.client, null,
                SERVICE_INTENT, null, harness.connection, 0, null, PACKAGE, 0) == 1);
        RecordingConnection systemConnection = new RecordingConnection();
        check(harness.active.bindService(SERVICE_INTENT, systemConnection) == 1);
        IBinder.DeathRecipient death = onlyConnectionDeath(harness.active, harness.connection);

        death.binderDied();

        check(connectionRecords(harness.active, harness.connection).isEmpty());
        check(indexDeath(harness.active, harness.connection) == null);
        check(connectionRecords(harness.active, systemConnection).size() == 1);
        check(indexDeath(harness.active, systemConnection) != null);
        check(onlyBinding(service).connections.size() == 1);
        check(!service.retiring);
        check(harness.launches == 0);

        // A delayed obituary from the retired registration must not detach a
        // new registration for the same client Binder after the death pin is
        // replaced.  Only the current recipient may remove the new row.
        check(harness.active.bindServiceInstance(42, 10042, harness.client, null,
                SERVICE_INTENT, null, harness.connection, 0, null, PACKAGE, 0) == 1);
        IBinder.DeathRecipient replacementDeath =
                onlyConnectionDeath(harness.active, harness.connection);
        check(replacementDeath != death);
        death.binderDied();
        check(connectionRecords(harness.active, harness.connection).size() == 1);
        replacementDeath.binderDied();
        check(connectionRecords(harness.active, harness.connection).isEmpty());

        ApplicationProcessRegistry.AttachedApplication gone = harness.processes.retireAttached(
                harness.attached.pid, harness.attached.startSequence, harness.owner);
        check(gone != null);
        harness.active.onProcessGone(gone);
        check(harness.launches == 1);
        check(onlyBinding(service).connections.size() == 1);
    }

    private static void publicationFailureRemovesDeadClient() throws Exception {
        Harness harness = new Harness();
        ServiceRecord service = harness.bind();
        harness.connection.failConnected = true;

        harness.active.publishService(41, service.token, SERVICE_INTENT, new Binder());

        check(connectionRecords(harness.active, harness.connection).isEmpty());
        check(indexDeath(harness.active, harness.connection) == null);
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

        ApplicationProcessRegistry.AttachedApplication gone = harness.processes.retireAttached(
                harness.attached.pid, harness.attached.startSequence, harness.owner);
        check(gone != null);
        harness.active.onProcessGone(gone);
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
        ApplicationProcessRegistry.AttachedApplication replacementSnapshot =
                harness.attachBoundServiceOwner(41, replacement, 2);
        harness.active.onProcessAttached(replacementSnapshot);
        check(service.applicationThread == replacement);
        check(replacement.creates == 1 && replacement.binds == 1);

        // The stale snapshot must not detach the replacement owner.
        harness.active.onProcessGone(harness.attached);
        check(service.applicationThread == replacement);
        check(harness.launches == 1);

        check(harness.processes.retireAttached(41, 2, replacement) != null);
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
        ApplicationProcessRegistry.AttachedApplication createGone = create.processes.retireAttached(
                create.attached.pid, create.attached.startSequence, create.owner);
        check(createGone == null); // Confirmed terminal transport already retired this incarnation.
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

    private static void linkReentryRetiresPendingClientBeforeSuccessfulLink() throws Exception {
        Harness harness = new Harness();
        RecordingConnection reentrant = new RecordingConnection();
        reentrant.beforeLink = () -> {
            ApplicationProcessRegistry.AttachedApplication gone = harness.processes.retireAttached(
                    42, 2, harness.client);
            check(gone != null);
            harness.active.onProcessGone(gone);
        };

        check(harness.active.bindServiceInstance(42, 10042, harness.client, null,
                SERVICE_INTENT, null, reentrant, 0, null, PACKAGE, 0) == 0);
        check(connectionRecords(harness.active, reentrant).isEmpty());
        check(indexDeath(harness.active, reentrant) == null);
        check(reentrant.links == 1 && reentrant.unlinks == 1);
        check(mapSize(harness.active, "servicesByToken") == 0);
        check(harness.launches == 0);
    }

    private static void blockedLinkRetirementFinishesBeforeLinkRelease() throws Exception {
        Harness harness = new Harness();
        RecordingConnection pending = new RecordingConnection();
        CountDownLatch linkEntered = new CountDownLatch(1);
        CountDownLatch releaseLink = new CountDownLatch(1);
        CountDownLatch goneFinished = new CountDownLatch(1);
        AtomicReference<Throwable> bindFailure = new AtomicReference<>();
        AtomicReference<Throwable> goneFailure = new AtomicReference<>();
        pending.beforeLink = () -> {
            check(!Thread.holdsLock(harness.active));
            linkEntered.countDown();
            try {
                check(releaseLink.await(2, TimeUnit.SECONDS));
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw new AssertionError(error);
            }
        };
        pending.beforeUnlink = () -> check(!Thread.holdsLock(harness.active));
        Thread binder = new Thread(() -> {
            try {
                check(harness.active.bindServiceInstance(42, 10042, harness.client, null,
                        SERVICE_INTENT, null, pending, 0, null, PACKAGE, 0) == 0);
            } catch (Throwable error) {
                bindFailure.set(error);
            }
        });
        binder.start();
        check(linkEntered.await(2, TimeUnit.SECONDS));

        ApplicationProcessRegistry.AttachedApplication gone = harness.processes.retireAttached(
                42, 2, harness.client);
        check(gone != null);
        Thread obituary = new Thread(() -> {
            try {
                harness.active.onProcessGone(gone);
            } catch (Throwable error) {
                goneFailure.set(error);
            } finally {
                goneFinished.countDown();
            }
        });
        obituary.start();
        // The process obituary must be able to detach the pending bind while
        // the Binder link call is still blocked in another thread.
        check(goneFinished.await(500, TimeUnit.MILLISECONDS));
        check(goneFailure.get() == null);
        check(harness.owner.creates == 0 && harness.owner.binds == 0);
        check(harness.launches == 0);

        releaseLink.countDown();
        binder.join(2_000);
        obituary.join(2_000);
        check(!binder.isAlive() && !obituary.isAlive());
        check(bindFailure.get() == null);
        check(pending.links == 1 && pending.unlinks == 1);
        check(connectionRecords(harness.active, pending).isEmpty());
        check(indexDeath(harness.active, pending) == null);
        check(mapSize(harness.active, "servicesByToken") == 0);
        check(harness.owner.creates == 0 && harness.owner.binds == 0);
        check(harness.launches == 0);
    }

    private static void runtimeLinkFailurePropagatesWithoutDeath() throws Exception {
        Harness harness = new Harness();
        RecordingConnection failing = new RecordingConnection();
        RuntimeException expected = new IllegalStateException("link callback failure");
        failing.linkFailure = expected;
        Throwable actual = null;
        try {
            harness.active.bindServiceInstance(42, 10042, harness.client, null, SERVICE_INTENT,
                    null, failing, 0, null, PACKAGE, 0);
        } catch (Throwable error) {
            actual = error;
        }
        check(actual == expected);
        // An unchecked error does not prove whether the recipient was installed.
        // Cleanup attempts only this exact recipient; it does not invent death.
        check(failing.links == 1 && failing.unlinks == 1);
        check(harness.processes.hasCallerIncarnation(42, 2, harness.client, 10042));
        check(connectionRecords(harness.active, failing).isEmpty());
        check(indexDeath(harness.active, failing) == null);
        check(harness.owner.creates == 0 && harness.owner.binds == 0);
        check(harness.launches == 0);
    }

    private static void synchronousConnectionDeathClosesLateSuccessfulLink() throws Exception {
        Harness harness = new Harness();
        RecordingConnection connection = new RecordingConnection();
        connection.dieBeforeInstall = true;
        connection.beforeUnlink = () -> check(!Thread.holdsLock(harness.active));
        check(harness.active.bindServiceInstance(42, 10042, harness.client, null, SERVICE_INTENT,
                null, connection, 0, null, PACKAGE, 0) == 0);
        check(connection.links == 1 && connection.unlinks == 1);
        check(indexDeath(harness.active, connection) == null);
        check(connectionRecords(harness.active, connection).isEmpty());
        check(mapSize(harness.active, "servicesByToken") == 0);
        check(harness.owner.creates == 0 && harness.launches == 0);
    }

    /** Goal-exit gate: legacy connected() must not keep an outer AS monitor. */
    private static void notificationReentryMustNotRetainActiveServicesMonitor() throws Exception {
        Harness harness = new Harness();
        ServiceRecord service = harness.bind();
        RecordingConnection nested = new RecordingConnection();
        nested.beforeLink = () -> check(!Thread.holdsLock(harness.active));
        harness.connection.connectedCallback = published -> {
            check(harness.active.bindServiceInstance(42, 10042, harness.client, null, SERVICE_INTENT,
                    null, nested, 0, null, PACKAGE, 0) == 1);
        };
        harness.active.publishService(41, service.token, SERVICE_INTENT, new Binder());
        check(nested.links == 1);
    }

    private static void initialAndCachedNotificationsAreUnlockedAndDeduplicated()
            throws Exception {
        Harness harness = new Harness();
        RecordingConnection nested = new RecordingConnection();
        nested.beforeLink = () -> check(!Thread.holdsLock(harness.active));
        nested.connectedCallback = ignored -> check(!Thread.holdsLock(harness.active));
        harness.connection.connectedCallback = published -> {
            check(!Thread.holdsLock(harness.active));
            if (harness.connection.connected == 1) {
                try {
                    check(harness.active.bindServiceInstance(42, 10042, harness.client, null,
                            SERVICE_INTENT, null, nested, 0, null, PACKAGE, 0) == 1);
                } catch (RemoteException error) {
                    throw new AssertionError(error);
                }
            }
        };
        harness.owner.createCallback = token -> harness.active.serviceDoneExecuting(
                41, 10042, token, 0, 0, 0, null);
        harness.owner.bindCallback = (token, intent) -> harness.active.publishService(
                41, token, intent, new Binder());

        ServiceRecord service = harness.bind();
        IntentBindRecord binding = onlyBinding(service);
        check(harness.connection.connected == 1);
        check(nested.links == 1 && nested.connected == 1);
        check(harness.owner.creates == 1 && harness.owner.binds == 1);
        check(binding.publicationReceived && !binding.bindCallbackPending);
        check(service.executingCallbacks == 0);

        RecordingConnection cached = new RecordingConnection();
        cached.beforeLink = () -> check(!Thread.holdsLock(harness.active));
        cached.connectedCallback = ignored -> check(!Thread.holdsLock(harness.active));
        check(harness.active.bindServiceInstance(42, 10042, harness.client, null, SERVICE_INTENT,
                null, cached, 0, null, PACKAGE, 0) == 1);
        check(cached.links == 1 && cached.connected == 1);
        check(harness.owner.binds == 1);

        // A second publication is not a second connected() notification and
        // does not disturb the already-settled genuine BIND callback.
        harness.active.publishService(41, service.token, SERVICE_INTENT, new Binder());
        check(harness.connection.connected == 1 && nested.connected == 1 && cached.connected == 1);
        check(service.executingCallbacks == 0 && binding.publicationReceived);
    }

    private static void detachedNotificationCannotDeleteReplacementRegistration()
            throws Exception {
        Harness harness = new Harness();
        CountDownLatch oldNotificationEntered = new CountDownLatch(1);
        CountDownLatch releaseOldNotification = new CountDownLatch(1);
        AtomicBoolean firstNotification = new AtomicBoolean(true);
        AtomicReference<Throwable> initialFailure = new AtomicReference<>();
        harness.connection.connectedCallback = published -> {
            check(!Thread.holdsLock(harness.active));
            if (!firstNotification.compareAndSet(true, false)) return;
            oldNotificationEntered.countDown();
            try {
                check(releaseOldNotification.await(2, TimeUnit.SECONDS));
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw new AssertionError(error);
            }
            throw new DeadObjectException();
        };
        harness.owner.createCallback = token -> harness.active.serviceDoneExecuting(
                41, 10042, token, 0, 0, 0, null);
        harness.owner.bindCallback = (token, intent) -> harness.active.publishService(
                41, token, intent, new Binder());

        Thread initialBind = new Thread(() -> {
            try {
                harness.bind();
            } catch (Throwable error) {
                initialFailure.set(error);
            }
        });
        initialBind.start();
        check(oldNotificationEntered.await(2, TimeUnit.SECONDS));

        // Supersede the old notification pin while its Binder call is still
        // in flight, then admit a replacement registration on the same Binder.
        check(harness.active.unbindService(harness.connection.asBinder()));
        check(harness.active.bindServiceInstance(42, 10042, harness.client, null, SERVICE_INTENT,
                null, harness.connection, 0, null, PACKAGE, 0) == 1);
        check(connectionRecords(harness.active, harness.connection).size() == 1);
        check(harness.connection.connected == 2);

        releaseOldNotification.countDown();
        initialBind.join(2_000);
        check(!initialBind.isAlive());
        check(initialFailure.get() == null);
        // The old RemoteException may retire only its captured record; the
        // replacement registration remains live and indexed.
        check(connectionRecords(harness.active, harness.connection).size() == 1);
        check(indexDeath(harness.active, harness.connection) != null);
        check(harness.owner.creates == 1 && harness.owner.binds == 1);
    }

    private static void pendingNotificationDetachedBeforeClaimIsCancelled() throws Exception {
        Harness harness = new Harness();
        harness.owner.createCallback = token -> harness.active.serviceDoneExecuting(
                41, 10042, token, 0, 0, 0, null);
        harness.owner.bindCallback = (token, intent) -> harness.active.publishService(
                41, token, intent, new Binder());
        ServiceRecord service = harness.bind();
        check(harness.connection.connected == 1);

        RecordingConnection pending = new RecordingConnection();
        pending.beforeLink = () -> {
            check(!Thread.holdsLock(harness.active));
            try {
                // Remove the exact pending record while its death link is still
                // LINKING; finishPendingBind must not claim a notification.
                check(harness.active.unbindService(pending.asBinder()));
            } catch (RemoteException error) {
                throw new AssertionError(error);
            }
        };
        check(harness.active.bindServiceInstance(42, 10042, harness.client, null, SERVICE_INTENT,
                null, pending, 0, null, PACKAGE, 0) == 0);
        check(pending.links == 1 && pending.unlinks == 1);
        check(pending.connected == 0);
        check(connectionRecords(harness.active, pending).isEmpty());
        check(indexDeath(harness.active, pending) == null);
        check(onlyBinding(service).hasAdmittedConnections());
        check(harness.owner.creates == 1 && harness.owner.binds == 1);
    }

    private static void admittedNotificationDetachedBeforeClaimIsCancelled() throws Exception {
        Harness harness = new Harness();
        harness.owner.createCallback = token -> harness.active.serviceDoneExecuting(
                41, 10042, token, 0, 0, 0, null);
        ServiceRecord service = harness.bind();
        RecordingConnection second = new RecordingConnection();
        check(harness.active.bindServiceInstance(42, 10042, harness.client, null, SERVICE_INTENT,
                null, second, 0, null, PACKAGE, 0) == 1);
        check(second.links == 1 && second.connected == 0);
        harness.connection.connectedCallback = published -> {
            check(!Thread.holdsLock(harness.active));
            check(harness.active.unbindService(second.asBinder()));
        };
        harness.active.publishService(41, service.token, SERVICE_INTENT, new Binder());
        check(harness.connection.connected == 1 && second.connected == 0);
        check(second.unlinks == 1 && connectionRecords(harness.active, second).isEmpty());
        check(service.executingCallbacks == 0);
    }

    private static void nullPublicationNotifiesOnceAndSettlesBindSeparately()
            throws Exception {
        Harness harness = new Harness();
        harness.connection.connectedCallback = published -> {
            check(!Thread.holdsLock(harness.active));
            check(published == null);
        };
        harness.owner.createCallback = token -> harness.active.serviceDoneExecuting(
                41, 10042, token, 0, 0, 0, null);
        harness.owner.bindCallback = (token, intent) -> harness.active.publishService(
                41, token, intent, null);
        ServiceRecord service = harness.bind();
        IntentBindRecord binding = onlyBinding(service);
        check(harness.connection.connected == 1);
        check(binding.publicationReceived && binding.publishedBinder == null);
        check(!binding.bindCallbackPending && service.executingCallbacks == 0);

        RecordingConnection cached = new RecordingConnection();
        cached.connectedCallback = published -> {
            check(!Thread.holdsLock(harness.active));
            check(published == null);
        };
        check(harness.active.bindServiceInstance(42, 10042, harness.client, null, SERVICE_INTENT,
                null, cached, 0, null, PACKAGE, 0) == 1);
        check(cached.connected == 1 && harness.owner.binds == 1);
        check(service.executingCallbacks == 0 && !binding.bindCallbackPending);

        harness.active.publishService(41, service.token, SERVICE_INTENT, null);
        check(harness.connection.connected == 1 && cached.connected == 1);
        check(service.executingCallbacks == 0);
    }

    private static void replacementPublicationWaitsForOldNotificationTail() throws Exception {
        replacementPublicationWaitsForOldNotificationTail(0);
        replacementPublicationWaitsForOldNotificationTail(1);
        replacementPublicationWaitsForOldNotificationTail(2);
    }

    private static void replacementPublicationWaitsForOldNotificationTail(int failureKind)
            throws Exception {
        Harness harness = new Harness();
        CountDownLatch oldEntered = new CountDownLatch(1);
        CountDownLatch releaseOld = new CountDownLatch(1);
        CountDownLatch newDelivered = new CountDownLatch(1);
        AtomicBoolean first = new AtomicBoolean(true);
        AtomicReference<Throwable> initialFailure = new AtomicReference<>();
        RuntimeException unchecked = new IllegalStateException("retained old unchecked failure");
        harness.connection.connectedCallback = published -> {
            check(!Thread.holdsLock(harness.active));
            if (first.compareAndSet(true, false)) {
                oldEntered.countDown();
                try {
                    check(releaseOld.await(2, TimeUnit.SECONDS));
                } catch (InterruptedException error) {
                    Thread.currentThread().interrupt();
                    throw new AssertionError(error);
                }
                DeadObjectException failure = new DeadObjectException();
                if (failureKind == 2) throw unchecked;
                if (failureKind == 1) failure.addSuppressed(new IllegalStateException("retained old cause"));
                throw failure;
            }
            newDelivered.countDown();
        };
        harness.owner.createCallback = token -> harness.active.serviceDoneExecuting(
                41, 10042, token, 0, 0, 0, null);
        harness.owner.bindCallback = (token, intent) -> harness.active.publishService(
                41, token, intent, new Binder());
        Thread initialBind = new Thread(() -> {
            try {
                harness.bind();
            } catch (Throwable error) {
                initialFailure.set(error);
            }
        });
        initialBind.start();
        check(oldEntered.await(2, TimeUnit.SECONDS));

        ApplicationProcessRegistry.AttachedApplication old = harness.processes.retireAttached(
                41, 1, harness.owner);
        check(old != null);
        harness.active.onProcessGone(old);
        IntentBindRecord invalidated = onlyBinding(onlyService(harness.active, "servicesByToken"));
        check(invalidated.notification.desired == null);
        check(invalidated.notification.delivered.isEmpty());
        check(invalidated.notification.inFlight.size() == 1);
        RecordingThread replacement = new RecordingThread();
        replacement.createCallback = token -> harness.active.serviceDoneExecuting(
                51, 10042, token, 0, 0, 0, null);
        replacement.bindCallback = (token, intent) -> harness.active.publishService(
                51, token, intent, new Binder());
        ApplicationProcessRegistry.AttachedApplication replacementSnapshot =
                harness.attachBoundServiceOwner(51, replacement, 2);
        harness.active.onProcessAttached(replacementSnapshot);

        // The replacement publication is current, but the same connection's
        // old notification tail must finish first; no out-of-order delivery.
        check(!newDelivered.await(200, TimeUnit.MILLISECONDS));
        check(harness.connection.connected == 1);
        check(replacement.creates == 1 && replacement.binds == 1);
        check(harness.owner.creates == 1 && harness.owner.binds == 1);

        releaseOld.countDown();
        initialBind.join(2_000);
        check(!initialBind.isAlive());
        check(initialFailure.get() == (failureKind == 2 ? unchecked : null));
        check(newDelivered.await(2, TimeUnit.SECONDS));
        check(harness.connection.connected == 2);
        check(connectionRecords(harness.active, harness.connection).size() == 1);
        check(indexDeath(harness.active, harness.connection) != null);
        check(harness.activeOwnerIs(replacement));
        ServiceRecord service = onlyService(harness.active, "servicesByToken");
        check(service.executingCallbacks == 0);

        // A late old tail cannot consume the replacement publication stamp or
        // schedule a third notification.
        harness.active.publishService(51, service.token, SERVICE_INTENT, new Binder());
        check(harness.connection.connected == 2);
    }

    private static void linkErrorPreservesFailedResourceTail() throws Exception {
        Harness harness = new Harness();
        RecordingConnection connection = new RecordingConnection();
        RuntimeException primary = new IllegalStateException("link failure");
        RuntimeException cleanup = new IllegalStateException("unlink failure");
        connection.linkFailure = primary;
        connection.unlinkFailureOnce = cleanup;
        Throwable actual = null;
        try {
            harness.active.bindServiceInstance(42, 10042, harness.client, null, SERVICE_INTENT,
                    null, connection, 0, null, PACKAGE, 0);
        } catch (Throwable failure) {
            actual = failure;
        }
        check(actual == primary);
        check(primary.getSuppressed().length == 1 && primary.getSuppressed()[0] == cleanup);
        check(connection.links == 1 && connection.unlinks == 2);
        check(harness.owner.creates == 0 && harness.launches == 0);
    }

    private static void failedLinkPreservesPendingUnbindToken() throws Exception {
        Harness harness = new Harness();
        ServiceRecord service = harness.bind();
        IntentBindRecord binding = onlyBinding(service);
        // Model an already-issued unbind callback while its service token is
        // still retained by the asynchronous create/bind work.
        binding.unbindScheduled = true;
        binding.bindScheduled = true;
        service.createScheduled = true;
        service.executingCallbacks = 1;
        service.retiring = false;

        RecordingConnection pending = new RecordingConnection();
        pending.failLink = true;
        pending.beforeLink = () -> {
            ApplicationProcessRegistry.AttachedApplication gone = harness.processes.retireAttached(
                    42, 2, harness.client);
            check(gone != null);
            harness.active.onProcessGone(gone);
        };
        check(harness.active.bindServiceInstance(42, 10042, harness.client, null,
                SERVICE_INTENT, null, pending, 0, null, PACKAGE, 0) == 0);

        check(connectionRecords(harness.active, pending).isEmpty());
        check(indexDeath(harness.active, pending) == null);
        check(pending.links == 1 && pending.unlinks == 0);
        check(mapSize(harness.active, "servicesByToken") == 1);
        check(onlyService(harness.active, "servicesByToken") == service);
        check(service.bindings.get(new Intent.FilterComparison(SERVICE_INTENT)) == binding);
        check(binding.connections.isEmpty());

        harness.active.unbindFinished(41, service.token, SERVICE_INTENT);
        check(mapSize(harness.active, "servicesByToken") == 1);
    }

    private static void synchronousLifecycleCompletionsSettleExactlyOnce() throws Exception {
        Harness harness = new Harness();
        harness.owner.createCallback = token -> harness.active.serviceDoneExecuting(
                41, 10042, token, 0, 0, 0, null);
        harness.owner.bindCallback = (token, intent) -> harness.active.publishService(
                41, token, intent, new Binder());
        ServiceRecord service = harness.bind();
        IntentBindRecord binding = onlyBinding(service);
        check(harness.owner.creates == 1 && harness.owner.binds == 1);
        check(service.executingCallbacks == 0);
        check(service.createScheduled && binding.bindScheduled && binding.hasBound);
        check(binding.publicationReceived && binding.publishedBinder != null);

        // A late duplicate completion is harmless and cannot resurrect the
        // operation count or mutate its reservation flags.
        harness.active.serviceDoneExecuting(41, 10042, service.token, 0, 0, 0, null);
        check(service.executingCallbacks == 0);
        check(service.createScheduled && binding.bindScheduled && binding.hasBound);

        // Two rows on one binding must still reserve exactly one unbind
        // operation when the shared client Binder is detached as a batch.
        check(harness.active.bindServiceInstance(42, 10042, harness.client, null,
                SERVICE_INTENT, null, harness.connection, 0, null, PACKAGE, 0) == 1);
        check(connectionRecords(harness.active, harness.connection).size() == 2);
        check(binding.connections.size() == 2);

        harness.owner.unbindCallback = (token, intent) -> harness.active.unbindFinished(
                41, token, intent);
        harness.owner.stopCallback = token -> harness.active.serviceDoneExecuting(
                41, 10042, token, 2, 0, 0, null);
        check(harness.active.unbindService(harness.connection.asBinder()));
        check(harness.owner.unbinds == 1 && harness.owner.stops == 1);
        check(service.executingCallbacks == 0);
        check(mapSize(harness.active, "servicesByToken") == 0);
        check(service.stopScheduled && service.createScheduled);
    }

    private static void synchronousBatchUnbindRetainsOnePendingStop() throws Exception {
        Harness harness = new Harness();
        harness.owner.createCallback = token -> harness.active.serviceDoneExecuting(
                41, 10042, token, 0, 0, 0, null);
        harness.owner.bindCallback = (token, intent) -> harness.active.publishService(
                41, token, intent, new Binder());
        ServiceRecord service = harness.bind();
        check(harness.active.bindServiceInstance(42, 10042, harness.client, null,
                SERVICE_INTENT, null, harness.connection, 0, null, PACKAGE, 0) == 1);
        harness.owner.unbindCallback = (token, intent) -> harness.active.unbindFinished(
                41, token, intent);
        // STOP stays pending, so the second detached row still sees the same
        // canonical token/binding after synchronous UNBIND completion.
        check(harness.active.unbindService(harness.connection.asBinder()));
        check(harness.owner.unbinds == 1 && harness.owner.stops == 1);
        check(service.executingCallbacks == 1);
        check(mapSize(harness.active, "servicesByToken") == 1);
        harness.active.serviceDoneExecuting(41, 10042, service.token, 2, 0, 0, null);
        check(mapSize(harness.active, "servicesByToken") == 0);
    }

    private static void stalePublicationCannotCompleteReplacementCallbacks() throws Exception {
        Harness harness = new Harness();
        ServiceRecord service = harness.bind();
        harness.active.serviceDoneExecuting(41, 10042, service.token, 0, 0, 0, null);
        harness.connection.connectedCallback = ignored -> {
            ApplicationProcessRegistry.AttachedApplication gone = harness.processes.retireAttached(
                    41, 1, harness.owner);
            check(gone != null);
            harness.active.onProcessGone(gone);
            RecordingThread replacement = new RecordingThread();
            harness.active.onProcessAttached(harness.attachBoundServiceOwner(
                    41, replacement, 2));
        };
        harness.active.publishService(41, service.token, SERVICE_INTENT, new Binder());
        check(service.ownerStartSequence == 2);
        // The old publication must not finish either new CREATE/BIND slot.
        check(service.executingCallbacks == 2);
        check(!onlyBinding(service).publicationReceived);
    }

    private static void staleOwnerDispatchCannotClearReplacement() throws Exception {
        Harness harness = new Harness();
        RecordingThread[] replacement = new RecordingThread[1];
        harness.owner.createCallback = token -> {
            ApplicationProcessRegistry.AttachedApplication gone = harness.processes.retireAttached(
                    41, 1, harness.owner);
            check(gone != null);
            harness.active.onProcessGone(gone);
            replacement[0] = new RecordingThread();
            ApplicationProcessRegistry.AttachedApplication snapshot =
                    harness.attachBoundServiceOwner(41, replacement[0], 2);
            harness.active.onProcessAttached(snapshot);
            throw new RemoteException("old owner dispatch failed after replacement");
        };

        ServiceRecord service = harness.bind();
        check(replacement[0] != null);
        check(service.applicationThread == replacement[0]);
        check(service.ownerPid == 41 && service.ownerStartSequence == 2);
        check(replacement[0].creates == 1 && replacement[0].binds == 1);
        check(harness.owner.creates == 1);
        check(harness.launches == 1);
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

    private static ApplicationProcessRegistry.AttachedApplication snapshotUnchecked(int pid,
            IBinder thread, String packageName, String processName, int uid, long sequence) {
        try {
            return snapshot(pid, thread, packageName, processName, uid, sequence);
        } catch (Exception error) {
            throw new AssertionError(error);
        }
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

    @SuppressWarnings("unchecked")
    private static ServiceConnectionIndex connectionIndex(ActiveServices active) {
        try {
            Field member = ActiveServices.class.getDeclaredField("connectionIndex");
            member.setAccessible(true);
            return (ServiceConnectionIndex) member.get(active);
        } catch (ReflectiveOperationException error) {
            throw new AssertionError(error);
        }
    }

    private static java.util.ArrayList<ConnectionRecord> connectionRecords(
            ActiveServices active, IBinder binder) {
        return connectionIndex(active).records(binder);
    }

    @SuppressWarnings("unchecked")
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
    private static IBinder.DeathRecipient onlyConnectionDeath(
            ActiveServices active, IBinder binder) {
        IBinder.DeathRecipient death = indexDeath(active, binder);
        check(death != null);
        return death;
    }

    private static IBinder.DeathRecipient indexDeath(ActiveServices active, IBinder binder) {
        return connectionIndex(active).death(binder);
    }

    private static void check(boolean value) {
        if (!value) throw new AssertionError();
    }
}
