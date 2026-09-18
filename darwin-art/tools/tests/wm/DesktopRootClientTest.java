package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.Parcel;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.view.InputChannel;
import java.lang.reflect.Field;
import java.util.ArrayList;
import java.util.List;

/** Component tests for the process client; no native/runtime output is linked. */
public final class DesktopRootClientTest {
    private static final String DESCRIPTOR = "dev.darwinart.runtime.wm.IDesktopRoot";
    private static final int REGISTER = 1;
    private static final int FACT = 1;
    private static final int BIND = 2;
    private static final int BIND_VERSION_1 = 1;
    private static final int ACTIVATED = 1;
    private static final int RESIGNED = 2;
    private static final int CLOSED = 3;

    private static final class Binding {
        final int version;
        final IBinder token;
        final int display;

        Binding(int wireVersion, IBinder originalToken, int logicalDisplay) {
            version = wireVersion;
            token = originalToken;
            display = logicalDisplay;
        }
    }

    private static final class Capability extends Binder {
        final List<Long> serials = new ArrayList<Long>();
        final List<Binding> bindings = new ArrayList<Binding>();
        final List<String> order;
        boolean acknowledge = true;
        boolean acknowledgeBind = true;
        boolean throwAfterBind;
        Runnable duringBind;
        Runnable duringFact;
        IBinder decisionCallback;
        int decisionAttaches;
        Runnable duringDecisionAttach;

        Capability(List<String> callOrder) { order = callOrder; }

        @Override public boolean transact(int code, Parcel data, Parcel reply, int flags)
                throws RemoteException {
            if (code == IBinder.FIRST_CALL_TRANSACTION + 2) {
                check(DESCRIPTOR.equals(data.readInterfaceToken()), "attach descriptor");
                check(data.readInt() == 1, "attach version");
                IBinder callback = data.readStrongBinder();
                check(decisionCallback == null || decisionCallback == callback,
                        "observation retry replaced process decision callback");
                decisionCallback = callback;
                ++decisionAttaches;
                if (duringDecisionAttach != null) {
                    Runnable action = duringDecisionAttach;
                    duringDecisionAttach = null;
                    action.run();
                }
                reply.writeNoException();
                reply.writeInt(5000);
                reply.writeInt(1000);
                return true;
            }
            if (code == BIND) {
                check(DESCRIPTOR.equals(data.readInterfaceToken()), "bind descriptor");
                int version = data.readInt();
                IBinder token = data.readStrongBinder();
                int display = data.readInt();
                check(version == BIND_VERSION_1, "bind version");
                check(token != null, "bind original token");
                check(display == 0, "bind display");
                bindings.add(new Binding(version, token, display));
                order.add("BIND");
                if (duringBind != null) {
                    Runnable callback = duringBind;
                    duringBind = null;
                    callback.run();
                }
                if (throwAfterBind) throw new RemoteException("transport lost after bind");
                reply.writeNoException();
                reply.writeBoolean(acknowledgeBind);
                return true;
            }
            if (code != FACT) return false;
            check(DESCRIPTOR.equals(data.readInterfaceToken()), "fact descriptor");
            int kind = data.readInt();
            long incarnation = data.readLong();
            long serial = data.readLong();
            boolean key = data.readBoolean();
            check(kind >= ACTIVATED && kind <= CLOSED, "fact kind");
            check(incarnation != 0 && serial != 0, "fact identity");
            check(kind != ACTIVATED || key, "activated key");
            check(kind != RESIGNED || !key, "resigned key");
            serials.add(Long.valueOf(serial));
            order.add("FACT");
            if (duringFact != null) duringFact.run();
            reply.writeNoException();
            reply.writeBoolean(acknowledge);
            return true;
        }
    }

    private static final class Service extends Binder {
        int registrations;
        boolean throwAfterPublish;
        long incarnation;
        Binder lifetime;
        final List<String> order = new ArrayList<String>();
        final Capability capability = new Capability(order);

        @Override public boolean transact(int code, Parcel data, Parcel reply, int flags)
                throws RemoteException {
            if (code != REGISTER) return false;
            check(DESCRIPTOR.equals(data.readInterfaceToken()), "register descriptor");
            incarnation = data.readLong();
            lifetime = (Binder) data.readStrongBinder();
            ++registrations;
            order.add("REGISTER");
            reply.writeNoException();
            reply.writeStrongBinder(capability);
            if (throwAfterPublish) throw new RemoteException("transport lost after publish");
            return true;
        }
    }

    private static final class Native implements DesktopRootClient.NativeBridge {
        long target = 77L;
        long incarnation = 0x8000000000000001L;
        boolean observeResult = true;
        boolean rejectBeforeObserveReturns;
        int observes;
        int releases;
        List<String> order;
        DesktopRootClient callback;
        DesktopRootClient oldCallback;
        final DecisionBoundary decisionBoundary = new DecisionBoundary();

        Native() {}
        Native(List<String> callOrder) { order = callOrder; }

        @Override public long acquireTarget() { return target; }
        @Override public long targetIncarnation(long value) { return incarnation; }
        @Override public boolean observe(long value, DesktopRootClient client) {
            ++observes;
            if (order != null) order.add("OBSERVE");
            if (oldCallback == null) oldCallback = client;
            callback = client;
            if (rejectBeforeObserveReturns) {
                Handler.rejectPosts = true;
                client.onHostFact(ACTIVATED, incarnation, 1L, true);
                Handler.rejectPosts = false;
            }
            return observeResult;
        }
        @Override public void releaseTarget(long value) { ++releases; }
        @Override public DesktopRootKeyDecisionNative.Boundary keyDecisions() {
            return decisionBoundary;
        }
    }

    private static final class DecisionBoundary implements DesktopRootKeyDecisionNative.Boundary {
        long nextBinding = 900L;
        int attaches;
        int publishes;
        int closes;
        int publishResult = DesktopRootKeyDecisionNative.RETAINED;
        RuntimeException publishFailure;
        DesktopRootFocusDecisionClient reenterClient;
        DesktopRootFocusDecision reenterDecision;
        int reenterResult = DesktopRootKeyDecisionNative.RETAINED;

        @Override public long attach(long binding, long target, long incarnation, IBinder capability) {
            check(target != 0L && incarnation != 0L && capability != null,
                    "native key attach identity");
            ++attaches;
            return binding != 0L ? binding : nextBinding++;
        }

        @Override public int publish(long binding, DesktopRootFocusDecision decision) {
            check(binding != 0L && decision != null, "native key publish identity");
            ++publishes;
            if (reenterClient != null) {
                DesktopRootFocusDecisionClient client = reenterClient;
                DesktopRootFocusDecision next = reenterDecision;
                reenterClient = null;
                int result = publishResult;
                publishResult = reenterResult;
                try {
                    client.onDecision(next);
                } finally {
                    publishResult = result;
                }
            }
            if (publishFailure != null) throw publishFailure;
            return publishResult;
        }

        @Override public void close(long binding) {
            check(binding != 0L, "native key close identity");
            ++closes;
        }
    }

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static InputChannel window(IBinder token) {
        return new InputChannel(token);
    }

    private static InputChannel window() {
        return window(new Binder());
    }

    private static void reset(Native nativeBridge, Service service) throws Exception {
        Looper.setMainLooper(new Looper());
        Handler.rejectPosts = false;
        setStatic("admissionClosed", Boolean.FALSE);
        setStatic("state", Enum.valueOf((Class) field("state").getType(), "IDLE"));
        setStatic("singleton", null);
        setStatic("registeredCapability", null);
        setStatic("registeredLifetime", null);
        setStatic("registeredIncarnation", Long.valueOf(0));
        setStatic("registeredDecisions", null);
        setStatic("running", Integer.valueOf(0));
        setStatic("lastSerial", Long.valueOf(0));
        setStatic("haveSerial", Boolean.FALSE);
        ((java.util.Set) field("PENDING").get(null)).clear();
        ((java.util.Set) field("BIND_PENDING").get(null)).clear();
        ((java.util.Set) field("BIND_DONE").get(null)).clear();
        setStatic("bindScheduled", Boolean.FALSE);
        setStatic("bindRequestGeneration", Long.valueOf(0));
        setStatic("mainHandler", null);
        setStatic("nativeBridge", nativeBridge);
        ServiceManager.setService(service);
    }

    private static Field field(String name) throws Exception {
        Field value = DesktopRootClient.class.getDeclaredField(name);
        value.setAccessible(true);
        return value;
    }

    private static void setStatic(String name, Object value) throws Exception {
        field(name).set(null, value);
    }

    private static void drain() throws Exception {
        Handler handler = (Handler) field("mainHandler").get(null);
        if (handler != null) handler.runAll();
    }

    private static int bindPendingCount() throws Exception {
        return ((java.util.Set) field("BIND_PENDING").get(null)).size();
    }

    private static void testCoalescedInitialSnapshot() throws Exception {
        Native nativeBridge = new Native();
        Service service = new Service();
        reset(nativeBridge, service);
        InputChannel channel = window();
        check(DesktopRootClient.ensureAttached(channel), "ensure did not schedule");
        check(DesktopRootClient.ensureAttached(channel), "coalesced ensure failed");
        check(service.registrations == 0, "Binder ran before main turn");
        drain();
        check(nativeBridge.observes == 1, "observe was not called once");
        nativeBridge.callback.onHostFact(ACTIVATED, nativeBridge.incarnation,
                Long.MIN_VALUE, true);
        drain();
        check(service.registrations == 1, "initial registration missing");
        check(service.capability.serials.size() == 1, "initial fact was not delivered");
    }

    private static void testRetryReusesCapability() throws Exception {
        Native nativeBridge = new Native();
        Service service = new Service();
        reset(nativeBridge, service);
        nativeBridge.observeResult = false;
        InputChannel channel = window();
        check(DesktopRootClient.ensureAttached(channel), "first ensure failed");
        drain();
        check(service.registrations == 1 && nativeBridge.releases == 1,
                "failed observe did not clean native target");
        nativeBridge.observeResult = true;
        check(DesktopRootClient.ensureAttached(channel), "retry was not allowed");
        drain();
        check(service.registrations == 1, "retry duplicated REGISTER");
        check(nativeBridge.observes == 2, "retry did not reuse capability");
    }

    private static void testUnsignedSerials() throws Exception {
        Native nativeBridge = new Native();
        Service service = new Service();
        reset(nativeBridge, service);
        DesktopRootClient.ensureAttached(window());
        drain();
        nativeBridge.callback.onHostFact(ACTIVATED, nativeBridge.incarnation,
                Long.MIN_VALUE, true);
        nativeBridge.callback.onHostFact(RESIGNED, nativeBridge.incarnation, -1L, false);
        nativeBridge.callback.onHostFact(ACTIVATED, nativeBridge.incarnation,
                Long.MIN_VALUE, true);
        drain();
        check(service.capability.serials.size() == 2, "signed serial ordering accepted stale fact");
    }

    private static void testCloseAdmissionRemovesQueuedFacts() throws Exception {
        Native nativeBridge = new Native();
        Service service = new Service();
        reset(nativeBridge, service);
        DesktopRootClient.ensureAttached(window());
        drain();
        nativeBridge.callback.onHostFact(ACTIVATED, nativeBridge.incarnation, 1L, true);
        DesktopRootClient.closeAdmission();
        check(DesktopRootClient.isQuiesced(), "queued fact survived close admission");
        check(!DesktopRootClient.ensureAttached(window()), "ensure accepted after close admission");
        drain();
        check(service.capability.serials.isEmpty(), "closed admission delivered fact");
    }

    private static void testMissingServiceCanRetry() throws Exception {
        Native nativeBridge = new Native();
        reset(nativeBridge, null);
        InputChannel channel = window();
        DesktopRootClient.ensureAttached(channel);
        drain();
        check(nativeBridge.releases == 1, "missing service leaked native target");
        Service service = new Service();
        ServiceManager.setService(service);
        check(DesktopRootClient.ensureAttached(channel), "service retry was not scheduled");
        drain();
        check(service.registrations == 1, "service retry did not register");
    }

    private static void testUncertainRegistrationIsTerminal() throws Exception {
        Native nativeBridge = new Native();
        Service service = new Service();
        service.throwAfterPublish = true;
        reset(nativeBridge, service);
        InputChannel channel = window();
        DesktopRootClient.ensureAttached(channel);
        drain();
        check(service.registrations == 1 && nativeBridge.releases == 1,
                "uncertain registration did not fail closed");
        check(!DesktopRootClient.ensureAttached(channel), "uncertain registration was retried");
    }

    private static void testRejectedPostRevokesObservation() throws Exception {
        Native nativeBridge = new Native();
        Service service = new Service();
        reset(nativeBridge, service);
        DesktopRootClient.ensureAttached(window());
        drain();
        Handler.rejectPosts = true;
        nativeBridge.callback.onHostFact(ACTIVATED, nativeBridge.incarnation, 1L, true);
        check(nativeBridge.releases == 1, "post rejection leaked native target");
    }

    private static void testPostRejectedBeforeObserveReturnsDoesNotResurrect() throws Exception {
        Native nativeBridge = new Native();
        Service service = new Service();
        reset(nativeBridge, service);
        nativeBridge.rejectBeforeObserveReturns = true;
        InputChannel channel = window();
        DesktopRootClient.ensureAttached(channel);
        drain();
        check(nativeBridge.releases == 1, "pre-return post rejection leaked target");
        nativeBridge.rejectBeforeObserveReturns = false;
        check(DesktopRootClient.ensureAttached(channel), "retry after pre-return rejection not allowed");
        drain();
        check(nativeBridge.observes == 2, "pre-return rejection resurrected old attempt");
        check(nativeBridge.releases == 1, "successful retry target was released");
    }

    private static void testLatePredecessorCannotRevokeSuccessor() throws Exception {
        Native nativeBridge = new Native();
        Service service = new Service();
        reset(nativeBridge, service);
        nativeBridge.observeResult = false;
        InputChannel channel = window();
        DesktopRootClient.ensureAttached(channel);
        drain();
        DesktopRootClient predecessor = nativeBridge.oldCallback;
        nativeBridge.observeResult = true;
        DesktopRootClient.ensureAttached(channel);
        drain();
        int releases = nativeBridge.releases;
        predecessor.onHostFact(ACTIVATED, nativeBridge.incarnation, 2L, true);
        drain();
        check(nativeBridge.releases == releases, "late predecessor revoked successor target");
        check(service.capability.serials.isEmpty(), "late predecessor delivered a fact");
    }

    private static void testClassLoadBeforeMainLooper() throws Exception {
        Looper.setMainLooper(null);
        Class.forName("dev.darwinart.runtime.wm.DesktopRootClient");
        check(!DesktopRootClient.ensureAttached(window()), "attachment accepted without main looper");
        DesktopRootClient.closeAdmission();
        check(DesktopRootClient.isQuiesced(), "no-looper closure needs queue draining");
    }

    private static void testCloseTracksRunningBinder() throws Exception {
        Native nativeBridge = new Native();
        Service service = new Service();
        reset(nativeBridge, service);
        DesktopRootClient.ensureAttached(window());
        drain();
        service.capability.duringFact = () -> {
            DesktopRootClient.closeAdmission();
            check(!DesktopRootClient.isQuiesced(), "running Binder work disappeared at close");
        };
        nativeBridge.callback.onHostFact(ACTIVATED, nativeBridge.incarnation, 1L, true);
        drain();
        check(DesktopRootClient.isQuiesced(), "completed Binder work did not settle");
    }

    private static void testInitialBindsPrecedeNativeObserveAndCaptureToken() throws Exception {
        Service service = new Service();
        Native nativeBridge = new Native(service.order);
        reset(nativeBridge, service);
        IBinder firstToken = new Binder();
        IBinder secondToken = new Binder();
        InputChannel first = window(firstToken);
        InputChannel second = window(secondToken);
        check(DesktopRootClient.ensureAttached(first), "first initial window was rejected");
        check(DesktopRootClient.ensureAttached(second), "second initial window was rejected");
        // The client must retain the original token, not reread a mutable
        // channel when the main-looper work eventually executes.
        first.setTokenForTest(new Binder());
        drain();
        check(nativeBridge.observes == 1, "initial observe missing");
        check(service.capability.bindings.size() == 2, "all initial windows were not bound");
        check(service.order.size() == 4, "unexpected initial transport sequence");
        check("REGISTER".equals(service.order.get(0)), "register did not precede initial binds");
        check("BIND".equals(service.order.get(1)) && "BIND".equals(service.order.get(2)),
                "initial binds were not contiguous");
        check("OBSERVE".equals(service.order.get(3)), "native observe preceded initial binds");
        Binding firstBinding = service.capability.bindings.get(0);
        Binding secondBinding = service.capability.bindings.get(1);
        check(firstBinding.version == BIND_VERSION_1 && firstBinding.display == 0,
                "first bind capability metadata");
        check(secondBinding.version == BIND_VERSION_1 && secondBinding.display == 0,
                "second bind capability metadata");
        check(firstBinding.token == firstToken, "bind reread the mutable first channel token");
        check(secondBinding.token == secondToken, "bind changed the second channel token");
    }

    private static void testLaterWindowsBindIndependentlyAfterRootAttached() throws Exception {
        Service service = new Service();
        Native nativeBridge = new Native();
        reset(nativeBridge, service);
        IBinder firstToken = new Binder();
        InputChannel first = window(firstToken);
        check(DesktopRootClient.ensureAttached(first), "root attachment was rejected");
        drain();
        int initialBinds = service.capability.bindings.size();
        check(initialBinds == 1, "initial window was not bound");

        IBinder laterToken = new Binder();
        check(DesktopRootClient.ensureAttached(window(laterToken)),
                "later window was not accepted while root attached");
        drain();
        check(nativeBridge.observes == 1, "later window restarted native observation");
        check(service.registrations == 1, "later window retried REGISTER");
        check(service.capability.bindings.size() == initialBinds + 1,
                "later window did not receive an independent BIND");
        check(service.capability.bindings.get(1).token == laterToken,
                "later BIND used the wrong original token");

        // A token is deduplicated only after an acknowledged exact tuple.
        check(DesktopRootClient.ensureAttached(window(laterToken)),
                "duplicate acknowledged token was rejected");
        drain();
        check(service.capability.bindings.size() == initialBinds + 1,
                "acknowledged duplicate token was rebound");
    }

    private static void testRejectedBindFalseIsRetryableWithoutFocusOrRegisterRetry()
            throws Exception {
        testRejectedBind(false);
    }

    private static void testRejectedBindThrowIsRetryableWithoutFocusOrRegisterRetry()
            throws Exception {
        testRejectedBind(true);
    }

    private static void testRejectedBind(boolean throwAfterBind) throws Exception {
        Service service = new Service();
        Native nativeBridge = new Native();
        reset(nativeBridge, service);
        InputChannel channel = window();
        service.capability.acknowledgeBind = false;
        service.capability.throwAfterBind = throwAfterBind;
        check(DesktopRootClient.ensureAttached(channel), "bind rejection attachment rejected");
        drain();
        check(service.registrations == 1, "bind rejection retried REGISTER");
        check(service.capability.bindings.size() == 1, "first rejected BIND missing");
        check(service.capability.serials.isEmpty(), "rejected BIND fabricated host focus fact");
        check(nativeBridge.observes == 1, "rejected BIND failed closed the root observer");

        service.capability.acknowledgeBind = true;
        service.capability.throwAfterBind = false;
        if (!throwAfterBind) {
            check(bindPendingCount() == 0, "terminal false BIND leaked a pending token");
            IBinder nextToken = new Binder();
            check(DesktopRootClient.ensureAttached(window(nextToken)),
                    "next window was rejected after terminal BIND");
            drain();
            check(service.registrations == 1, "window churn retried REGISTER");
            check(service.capability.bindings.size() == 2,
                    "next window did not receive its own BIND");
            check(service.capability.bindings.get(0).token != nextToken,
                    "terminal BIND was retried during window churn");
            check(service.capability.bindings.get(1).token == nextToken,
                    "window churn bound the wrong token");
            check(bindPendingCount() == 0, "terminal window churn left pending state");
            check(service.capability.serials.isEmpty(), "window churn fabricated host focus fact");
            return;
        }
        check(bindPendingCount() == 1, "lost BIND acknowledgement did not retain token");
        check(DesktopRootClient.ensureAttached(channel),
                "explicit receiver retry did not reschedule uncertain BIND");
        drain();
        check(service.registrations == 1, "explicit BIND retry retried REGISTER");
        check(service.capability.bindings.size() == 2,
                "uncertain BIND was marked done before acknowledgement");
        check(service.capability.bindings.get(0).token == service.capability.bindings.get(1).token,
                "BIND retry changed the original token tuple");
        check(bindPendingCount() == 0, "acknowledged BIND retry remained pending");
        check(service.capability.serials.isEmpty(), "BIND retry fabricated host focus fact");
    }

    private static void testBindGenerationTailAfterReceiverDuringBind() throws Exception {
        Service service = new Service();
        Native nativeBridge = new Native();
        reset(nativeBridge, service);
        IBinder firstToken = new Binder();
        IBinder laterToken = new Binder();
        service.capability.duringBind = () -> {
            check(DesktopRootClient.ensureAttached(window(laterToken)),
                    "receiver during BIND was rejected");
        };
        check(DesktopRootClient.ensureAttached(window(firstToken)),
                "initial receiver was rejected");
        drain();
        check(service.capability.bindings.size() == 2,
                "BIND generation tail did not deliver receiver added during BIND");
        check(service.capability.bindings.get(0).token == firstToken,
                "initial BIND generation used wrong token");
        check(service.capability.bindings.get(1).token == laterToken,
                "BIND generation tail used wrong token");
        check(bindPendingCount() == 0, "BIND generation tail left pending state");
    }

    private static void testAuthenticatedDecisionRetention() throws Exception {
        Native nativeBridge = new Native();
        Service service = new Service();
        reset(nativeBridge, service);
        IBinder original = new Binder();
        service.capability.duringDecisionAttach = () -> {
            try {
                Binder.setCallingIdentity(1234, 1000);
                DesktopRootFocusDecisionTransport.send(service.capability.decisionCallback,
                        new DesktopRootFocusDecision(nativeBridge.incarnation, 1,
                                Long.MAX_VALUE, original, 1));
                Binder.setCallingIdentity(5000, 1000);
                DesktopRootFocusDecisionTransport.send(service.capability.decisionCallback,
                        new DesktopRootFocusDecision(nativeBridge.incarnation, 1, 1, original, 1));
            } catch (RemoteException failure) { throw new RuntimeException(failure); }
            finally { Binder.setCallingIdentity(0, 0); }
        };
        DesktopRootClient.ensureAttached(window(original));
        drain();
        DesktopRootFocusDecisionClient retained =
                (DesktopRootFocusDecisionClient) field("registeredDecisions").get(null);
        check(retained.snapshot().sequence == 1
                && retained.snapshot().originalChannelToken == original,
                "early forged highwater poisoned trusted handshake retention");
        retained.attach(service.capability, nativeBridge.target);
        check(service.capability.lifetimeLinks == 1,
                "same-capability decision retry duplicated server lifetime link");
        Binder.setCallingIdentity(1234, 1000);
        check(!DesktopRootFocusDecisionTransport.send(service.capability.decisionCallback,
                new DesktopRootFocusDecision(nativeBridge.incarnation, 2, 99, null, 2)),
                "forged server callback was accepted after handshake");
        Binder.setCallingIdentity(5000, 1000);
        DesktopRootFocusDecision revoke =
                new DesktopRootFocusDecision(nativeBridge.incarnation, 2, 2, null, 2);
        check(DesktopRootFocusDecisionTransport.send(service.capability.decisionCallback, revoke),
                "authenticated revocation was rejected");
        check(retained.snapshot().originalChannelToken == null && retained.snapshot().epoch == 2,
                "client lost authenticated loss epoch");
        DesktopRootFocusDecision latest = retained.snapshot();
        boolean duplicateAccepted = DesktopRootFocusDecisionTransport.send(
                service.capability.decisionCallback, revoke);
        check(duplicateAccepted
                && retained.snapshot() == latest, "duplicate retention was not idempotent");
        Object clientLock = field("LOCK").get(null);
        service.capability.duringUnlink = () -> {
            check(!Thread.holdsLock(clientLock), "process lock spans Binder lifetime unlink");
            check(!DesktopRootClient.isQuiesced(), "external lifetime cleanup tail was untracked");
        };
        DesktopRootClient.closeAdmission();
        check(retained.snapshot() == null, "shutdown retained decision admission");
        check(service.capability.lifetimeUnlinks == 1,
                "shutdown did not release exact server lifetime link once");
        check(!retained.isServerPinned()
                && !DesktopRootFocusDecisionTransport.send(service.capability.decisionCallback,
                        new DesktopRootFocusDecision(nativeBridge.incarnation, 3, 3, original, 3)),
                "closed transport accepted or retained a late token grant");
        Binder.setCallingIdentity(0, 0);
    }

    private static void testNativeDecisionOutcomesAndSupersededFailure() throws Exception {
        Capability retryServer = new Capability(new ArrayList<String>());
        DecisionBoundary retryBoundary = new DecisionBoundary();
        DesktopRootFocusDecisionClient retryClient =
                new DesktopRootFocusDecisionClient(20, retryBoundary);
        retryClient.attach(retryServer, 101L);
        DesktopRootFocusDecision first =
                new DesktopRootFocusDecision(20, 1L, 1L, new Binder(), 1L);
        retryBoundary.publishResult = DesktopRootKeyDecisionNative.RETRY;
        boolean retry = false;
        try {
            retryClient.onDecision(first);
        } catch (IllegalStateException expected) {
            retry = true;
        }
        check(retry && retryClient.isNativeBound() && retryClient.snapshot() == first,
                "native RETRY closed or lost the durable Java decision");
        retryBoundary.publishResult = DesktopRootKeyDecisionNative.RETAINED;
        retryClient.onDecision(first);
        check(retryBoundary.publishes == 2 && retryClient.isNativeBound(),
                "same-record native retry did not reuse the exact binding");
        retryClient.closeAdmission();

        Capability failedServer = new Capability(new ArrayList<String>());
        DecisionBoundary failedBoundary = new DecisionBoundary();
        DesktopRootFocusDecisionClient failedClient =
                new DesktopRootFocusDecisionClient(21, failedBoundary);
        failedClient.attach(failedServer, 102L);
        failedBoundary.publishResult = DesktopRootKeyDecisionNative.FAILED;
        boolean failed = false;
        try {
            failedClient.onDecision(new DesktopRootFocusDecision(21, 1L, 1L,
                    new Binder(), 1L));
        } catch (IllegalStateException expected) {
            failed = true;
        }
        check(failed && failedBoundary.closes == 1 && !failedClient.isNativeBound()
                && failedClient.snapshot() == null,
                "native FAILED did not close the exact admission");

        Capability supersededServer = new Capability(new ArrayList<String>());
        DecisionBoundary supersededBoundary = new DecisionBoundary();
        DesktopRootFocusDecisionClient supersededClient =
                new DesktopRootFocusDecisionClient(22, supersededBoundary);
        supersededClient.attach(supersededServer, 103L);
        supersededBoundary.reenterClient = supersededClient;
        supersededBoundary.reenterDecision = new DesktopRootFocusDecision(22, 2L, 2L,
                new Binder(), 2L);
        supersededBoundary.reenterResult = DesktopRootKeyDecisionNative.RETAINED;
        supersededBoundary.publishResult = DesktopRootKeyDecisionNative.FAILED;
        supersededClient.onDecision(new DesktopRootFocusDecision(22, 1L, 1L,
                new Binder(), 1L));
        check(supersededClient.isNativeBound()
                && supersededClient.snapshot().sequence == 2L
                && supersededBoundary.closes == 0,
                "superseded native failure closed a newer successful decision");
        supersededClient.closeAdmission();
    }

    private static void testDecisionServerLifetime() throws Exception {
        Capability server = new Capability(new ArrayList<String>());
        DesktopRootFocusDecisionClient client =
                new DesktopRootFocusDecisionClient(7, new DecisionBoundary());
        client.attach(server, 1L);
        Binder.setCallingIdentity(5000, 1000);
        DesktopRootFocusDecisionTransport.send(client,
                new DesktopRootFocusDecision(7, 1, 1, new Binder(), 1));
        check(client.snapshot() != null, "live server decision missing");
        server.die();
        check(client.snapshot() == null && !client.isServerPinned(),
                "server death left retained input authority");
        check(!DesktopRootFocusDecisionTransport.send(client,
                new DesktopRootFocusDecision(7, 2, 2, new Binder(), 2)),
                "server-death late grant reopened admission");
        boolean rejected = false;
        try { client.attach(server, 1L); } catch (RemoteException expected) { rejected = true; }
        check(rejected && server.lifetimeLinks == 1,
                "dead server retry reopened or relinked authority");
        client.closeAdmission();
        check(server.lifetimeUnlinks == 1, "death/shutdown double-unlinked capability");

        for (boolean dieDuringLink : new boolean[] {false, true}) {
            Capability linkingServer = new Capability(new ArrayList<String>());
            DesktopRootFocusDecisionClient linkingClient =
                    new DesktopRootFocusDecisionClient(8, new DecisionBoundary());
            linkingServer.duringLink = () -> {
                if (dieDuringLink) linkingServer.die();
                else linkingClient.closeAdmission();
            };
            rejected = false;
            try { linkingClient.attach(linkingServer, 1L); }
            catch (RemoteException expected) { rejected = true; }
            check(rejected && linkingServer.decisionAttaches == 0
                    && linkingServer.lifetimeLinks == 1 && linkingServer.lifetimeUnlinks == 1
                    && linkingClient.snapshot() == null && !linkingClient.isServerPinned(),
                    "close/death during external lifetime link resurrected ATTACH/pin");
        }
        Capability alreadyDead = new Capability(new ArrayList<String>());
        alreadyDead.dead = true;
        DesktopRootFocusDecisionClient deadClient =
                new DesktopRootFocusDecisionClient(9, new DecisionBoundary());
        rejected = false;
        try { deadClient.attach(alreadyDead, 1L); } catch (RemoteException expected) { rejected = true; }
        check(rejected && alreadyDead.decisionAttaches == 0 && !deadClient.isServerPinned(),
                "failed lifetime link proceeded to trusted handshake");

        Capability handshakeServer = new Capability(new ArrayList<String>());
        DesktopRootFocusDecisionClient handshakeClient =
                new DesktopRootFocusDecisionClient(10, new DecisionBoundary());
        handshakeServer.duringDecisionAttach = () -> {
            try {
                DesktopRootFocusDecisionTransport.send(handshakeClient,
                        new DesktopRootFocusDecision(10, 1, 1, new Binder(), 1));
            } catch (RemoteException failure) { throw new RuntimeException(failure); }
            handshakeServer.die();
        };
        rejected = false;
        try { handshakeClient.attach(handshakeServer, 1L); }
        catch (RemoteException expected) { rejected = true; }
        check(rejected && handshakeClient.snapshot() == null && !handshakeClient.isServerPinned()
                && handshakeServer.lifetimeUnlinks == 1,
                "death between early callback and handshake pin retained grant");

        Capability concurrentServer = new Capability(new ArrayList<String>());
        DesktopRootFocusDecisionClient concurrentClient =
                new DesktopRootFocusDecisionClient(11, new DecisionBoundary());
        concurrentServer.duringLink = () -> {
            boolean deferred = false;
            try { concurrentClient.attach(concurrentServer, 1L); }
            catch (RemoteException expected) { deferred = true; }
            check(deferred && concurrentServer.decisionAttaches == 0,
                    "overlapping attach bypassed in-flight lifetime link");
        };
        concurrentClient.attach(concurrentServer, 1L);
        check(concurrentClient.isServerPinned() && concurrentServer.lifetimeLinks == 1,
                "overlapping retry double-linked or closed valid first attempt");
        concurrentClient.closeAdmission();
        Capability badCleanup = new Capability(new ArrayList<String>());
        DesktopRootFocusDecisionClient cleanupClient =
                new DesktopRootFocusDecisionClient(12, new DecisionBoundary());
        cleanupClient.attach(badCleanup, 1L);
        badCleanup.unlinkFailure = new IllegalStateException("fixture unlink transport failure");
        int beforeWarnings = android.util.Log.lifetimeWarnings;
        badCleanup.die();
        check(!cleanupClient.isServerPinned() && cleanupClient.snapshot() == null
                && android.util.Log.lifetimeWarnings == beforeWarnings + 1,
                "unlink failure interrupted terminal closure or lacked diagnostics");
        Binder.setCallingIdentity(0, 0);
    }

    public static void main(String[] args) throws Exception {
        testDecisionServerLifetime();
        testClassLoadBeforeMainLooper();
        testCoalescedInitialSnapshot();
        testRetryReusesCapability();
        testUnsignedSerials();
        testCloseAdmissionRemovesQueuedFacts();
        testMissingServiceCanRetry();
        testUncertainRegistrationIsTerminal();
        testRejectedPostRevokesObservation();
        testPostRejectedBeforeObserveReturnsDoesNotResurrect();
        testLatePredecessorCannotRevokeSuccessor();
        testCloseTracksRunningBinder();
        testInitialBindsPrecedeNativeObserveAndCaptureToken();
        testLaterWindowsBindIndependentlyAfterRootAttached();
        testRejectedBindFalseIsRetryableWithoutFocusOrRegisterRetry();
        testRejectedBindThrowIsRetryableWithoutFocusOrRegisterRetry();
        testBindGenerationTailAfterReceiverDuringBind();
        testAuthenticatedDecisionRetention();
        testNativeDecisionOutcomesAndSupersededFailure();
        System.out.println("DesktopRootClient main-looper, capability, ordering, and shutdown tests: PASS");
    }
}
