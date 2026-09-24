package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Process;
import android.os.RemoteException;
import android.view.IWindowManager;
import android.view.IWindowSession;
import android.view.InputChannel;
import android.view.View;
import android.view.WindowManager;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;
import java.lang.reflect.Field;
import java.util.ArrayList;
import java.util.List;

/** End-to-end WMS/root decision publication tests using the production owners. */
public final class DesktopRootFocusDecisionTest {
    private static final int PID = 4101;
    private static final int UID = 10101;
    private static final int FOREIGN_PID = 4102;
    private static final int FOREIGN_UID = 10102;
    private static int checks;

    private static final class Foreground implements DesktopForegroundAuthority.Provider {
        int foregroundPid = PID;
        @Override public DesktopForegroundAuthority.ProcessInstance capture(int pid) {
            return new DesktopForegroundAuthority.ProcessInstance(pid, 1, 0);
        }
        @Override public boolean isForeground(DesktopForegroundAuthority.ProcessInstance process) {
            return process != null && process.pid == foregroundPid;
        }
    }

    /** Binder callback that gives the production receiver a real server identity. */
    private static final class Callback extends DesktopRootFocusDecisionTransport.Receiver {
        final List<DesktopRootFocusDecision> seen = new ArrayList<>();
        final List<DesktopRootFocusDecision> attempted = new ArrayList<>();
        Object publications;
        Fixture fixture;
        boolean remoteFailure;
        boolean runtimeFailure;
        boolean reenterResign;
        boolean removeOnGrant;
        boolean checkedOutsideLock;
        private boolean reentered;

        @Override public boolean transact(int code, Parcel data, Parcel reply, int flags)
                throws RemoteException {
            // WindowRootFocusDecisions invokes Binder outside its controller monitor.  Keep
            // that property observable while still simulating a cross-process callback.
            if (remoteFailure) {
                readAttempt(data);
                throw new RemoteException("injected generic callback transport loss");
            }
            int oldPid = Binder.getCallingPid();
            int oldUid = Binder.getCallingUid();
            Binder.setCallingIdentity(Process.myPid(), Process.myUid());
            try { return super.transact(code, data, reply, flags); }
            finally { Binder.setCallingIdentity(oldPid, oldUid); }
        }

        private void readAttempt(Parcel data) {
            check(DesktopRootFocusDecisionTransport.DESCRIPTOR.equals(data.readInterfaceToken()),
                    "decision descriptor");
            check(data.readInt() == DesktopRootFocusDecisionTransport.VERSION,
                    "decision wire version");
            long incarnation = data.readLong();
            long serial = data.readLong();
            long sequence = data.readLong();
            IBinder token = data.readStrongBinder();
            long epoch = data.readLong();
            attempted.add(new DesktopRootFocusDecision(incarnation, serial, sequence, token, epoch));
        }

        @Override protected void onDecision(DesktopRootFocusDecision decision) {
            seen.add(decision);
            if (publications != null && !Thread.holdsLock(publications)) checkedOutsideLock = true;
            if (runtimeFailure) throw new IllegalStateException("injected callback failure");
            if (reenterResign && !reentered && decision.originalChannelToken != null) {
                reentered = true;
                fixture.fact(DesktopRootRegistry.RESIGNED, fixture.rootIncarnation, 2, false);
            }
            if (removeOnGrant && !reentered && decision.originalChannelToken != null) {
                reentered = true;
                fixture.removeWindow(fixture.window);
            }
        }
    }

    private static final class Fixture {
        final Foreground foreground = new Foreground();
        final ApplicationProcessRegistry processes = new ApplicationProcessRegistry();
        final WindowManagerEndpoint manager;
        final DesktopRootEndpoint roots;
        final Binder appThread = new Binder();
        final WindowSessionEndpoint session;
        final Binder window = new Binder();
        final InputChannel original;
        final long rootIncarnation = 0x1000000000000001L;
        final Binder lifetime = new Binder();
        final IBinder capability;
        final Object publications;

        Fixture() throws Exception {
            HandlerThread.resetQueues();
            Handler.allowPosts = -1;
            processes.beginAttachment(PID, UID, appThread, 1);
            processes.identify(PID, 1, appThread, "fixture.focus." + PID);
            Binder.setCallingIdentity(PID, UID);
            manager = new WindowManagerEndpoint(processes, new DesktopWindowMetadataRegistry(),
                new dev.darwinart.runtime.display.TaskDisplayRegistry());
            roots = new DesktopRootEndpoint(processes, manager.createDesktopRootRegistry(foreground));
            session = openSession();
            addWindow(window);
            original = channel(window);
            publications = field(manager, "publications");
            capability = register();
        }

        private WindowSessionEndpoint openSession() throws Exception {
            Parcel data = Parcel.obtain(), reply = Parcel.obtain();
            data.writeInterfaceToken("android.view.IWindowManager");
            data.writeStrongBinder(appThread);
            check(manager.transact(IWindowManager.Stub.TRANSACTION_openSession, data, reply, 0),
                    "openSession handled");
            reply.readException();
            return (WindowSessionEndpoint) reply.readStrongBinder();
        }

        private static WindowManager.LayoutParams attrs() {
            WindowManager.LayoutParams attrs = new WindowManager.LayoutParams();
            attrs.type = WindowManager.LayoutParams.FIRST_APPLICATION_WINDOW;
            attrs.width = 320;
            attrs.height = 240;
            return attrs;
        }

        void addWindow(Binder token) throws Exception {
            Binder.setCallingIdentity(PID, UID);
            Parcel data = Parcel.obtain(), reply = Parcel.obtain();
            data.writeInterfaceToken("android.view.IWindowSession");
            data.writeStrongBinder(token);
            data.writeTypedObject(attrs(), 0);
            data.writeInt(View.VISIBLE);
            data.writeInt(0); data.writeInt(0); data.writeInt(0); data.writeInt(1);
            check(session.transact(IWindowSession.Stub.TRANSACTION_addToDisplayAsUser,
                    data, reply, 0), "add handled");
            reply.readException();
            relayout(token);
        }

        private void relayout(Binder token) throws Exception {
            Binder.setCallingIdentity(PID, UID);
            Parcel data = Parcel.obtain(), reply = Parcel.obtain();
            data.writeInterfaceToken("android.view.IWindowSession");
            data.writeStrongBinder(token);
            data.writeTypedObject(attrs(), 0);
            data.writeInt(320); data.writeInt(240); data.writeInt(View.VISIBLE);
            data.writeInt(0); data.writeInt(1); data.writeInt(0);
            check(session.transact(IWindowSession.Stub.TRANSACTION_relayout, data, reply, 0),
                    "relayout handled");
            reply.readException();
        }

        void removeWindow(Binder token) {
            try {
                Binder.setCallingIdentity(PID, UID);
                Parcel data = Parcel.obtain(), reply = Parcel.obtain();
                data.writeInterfaceToken("android.view.IWindowSession");
                data.writeStrongBinder(token);
                check(session.transact(IWindowSession.Stub.TRANSACTION_remove, data, reply, 0),
                        "remove handled");
                reply.readException();
            } catch (Exception failure) {
                throw new AssertionError(failure);
            }
        }

        InputChannel channel(Binder token) throws Exception {
            @SuppressWarnings("unchecked")
            java.util.Map<WindowSessionWindowOwnership.Registration, InputChannel> channels =
                    (java.util.Map<WindowSessionWindowOwnership.Registration, InputChannel>)
                            field(session, "serverInputChannels");
            for (java.util.Map.Entry<WindowSessionWindowOwnership.Registration, InputChannel> e
                    : channels.entrySet()) {
                if (e.getKey().window().equals(token)) return e.getValue();
            }
            throw new AssertionError("missing original channel");
        }

        private IBinder register() throws Exception {
            Binder.setCallingIdentity(PID, UID);
            return DesktopRootProtocol.register(roots, rootIncarnation, lifetime);
        }

        Callback attachBeforeBind() throws Exception {
            return attach(capability, rootIncarnation);
        }

        Callback attach(IBinder rootCapability, long incarnation) throws Exception {
            Callback callback = new Callback();
            callback.fixture = this;
            callback.publications = publications;
            Binder.setCallingIdentity(PID, UID);
            DesktopRootProtocol.ServerIdentity server =
                    DesktopRootProtocol.attachFocusDecisions(rootCapability, callback);
            check(server.pid == Process.myPid() && server.uid == Process.myUid(),
                    "attach returns real server Process identity");
            check(server.pid != PID && server.uid != UID, "server identity differs from caller");
            check(callback.pinServer(server.pid, server.uid), "callback server pin");
            return callback;
        }

        void bind() throws Exception {
            Binder.setCallingIdentity(PID, UID);
            DesktopRootProtocol.bindWindow(capability, original.getToken(), 0);
        }

        void fact(int kind, long incarnation, long serial, boolean key) {
            try {
                Binder.setCallingIdentity(PID, UID);
                DesktopRootProtocol.fact(capability, kind, incarnation, serial, key);
            } catch (Exception failure) {
                throw new AssertionError(failure);
            }
        }

        void factOn(IBinder rootCapability, int kind, long incarnation, long serial,
                boolean key) {
            try {
                Binder.setCallingIdentity(PID, UID);
                DesktopRootProtocol.fact(rootCapability, kind, incarnation, serial, key);
            } catch (Exception failure) {
                throw new AssertionError(failure);
            }
        }
    }

    private static Object field(Object owner, String name) throws Exception {
        Field value = owner.getClass().getDeclaredField(name);
        value.setAccessible(true);
        return value.get(owner);
    }

    private static void check(boolean condition, String message) {
        ++checks;
        if (!condition) throw new AssertionError(message);
    }

    private static void pump() {
        for (int i = 0; i < 256 && HandlerThread.runAnyNext(); ++i) {}
    }

    private static DesktopRootFocusDecision last(Callback callback) {
        check(!callback.seen.isEmpty(), "callback has a decision");
        return callback.seen.get(callback.seen.size() - 1);
    }

    private static void rejected(Runnable operation, String message) {
        try {
            operation.run();
            throw new AssertionError(message);
        } catch (SecurityException | IllegalStateException expected) {
            ++checks;
        }
    }

    private static void testAttachBeforeBindAndNullEpoch() throws Exception {
        Fixture fixture = new Fixture();
        Callback callback = fixture.attachBeforeBind();
        pump();
        check(callback.seen.size() == 1, "initial attach publishes one snapshot");
        DesktopRootFocusDecision initial = last(callback);
        check(initial.originalChannelToken == null && initial.factSerial == 0
                        && initial.epoch == 0,
                "unbound attach publishes null-token epoch-zero revoke");
        fixture.bind();
        pump();
        check(callback.seen.size() == 1, "bind does not duplicate unchanged decision");
    }

    private static void testBindActivationGrantAndOriginalToken() throws Exception {
        Fixture fixture = new Fixture();
        Callback callback = fixture.attachBeforeBind();
        pump(); // settle the initial epoch-zero revoke before injecting transport loss
        fixture.bind();
        fixture.fact(DesktopRootRegistry.ACTIVATED, fixture.rootIncarnation, 7, true);
        pump();
        DesktopRootFocusDecision decision = last(callback);
        check(decision.originalChannelToken == fixture.original.getToken(),
                "grant carries exact original server channel token");
        check(decision.factSerial == 7 && decision.epoch > 0
                        && decision.incarnation == fixture.rootIncarnation,
                "grant carries retained fact serial and positive epoch");
    }

    private static void testIdenticalAttachAndRejects() throws Exception {
        Fixture fixture = new Fixture();
        Callback callback = fixture.attachBeforeBind();
        pump();
        int before = callback.seen.size();
        Binder.setCallingIdentity(PID, UID);
        DesktopRootProtocol.attachFocusDecisions(fixture.capability, callback);
        pump();
        check(callback.seen.size() == before + 1
                        && callback.seen.get(before).equals(callback.seen.get(before - 1)),
                "identical attach replays only exact latest decision");
        Callback replacement = new Callback();
        rejected(() -> {
            try { DesktopRootProtocol.attachFocusDecisions(fixture.capability, replacement); }
            catch (RemoteException failure) { throw new IllegalStateException(failure); }
        }, "different callback replacement accepted");
        Binder.setCallingIdentity(FOREIGN_PID, FOREIGN_UID);
        rejected(() -> {
            try { DesktopRootProtocol.attachFocusDecisions(fixture.capability, new Callback()); }
            catch (RemoteException failure) { throw new IllegalStateException(failure); }
        }, "foreign caller attach accepted");
        fixture.processes.retireAttached(PID, 1, fixture.appThread);
        Binder.setCallingIdentity(PID, UID);
        rejected(() -> {
            try { DesktopRootProtocol.attachFocusDecisions(fixture.capability, new Callback()); }
            catch (RemoteException failure) { throw new IllegalStateException(failure); }
        }, "stale caller attach accepted");
    }

    private static void testDisplacementRevokeAndGain() throws Exception {
        Fixture fixture = new Fixture();
        Callback callbackA = fixture.attachBeforeBind();
        fixture.bind();
        fixture.fact(DesktopRootRegistry.ACTIVATED, fixture.rootIncarnation, 1, true);
        pump();
        check(last(callbackA).originalChannelToken == fixture.original.getToken(),
                "first root gained focus");
        // Add a second canonical window and authenticated root. Binding ownership and focus
        // displacement remain the production controller's policy, not a fixture shortcut.
        Binder secondWindow = new Binder();
        fixture.addWindow(secondWindow);
        InputChannel secondOriginal = fixture.channel(secondWindow);
        Binder secondLifetime = new Binder();
        Binder.setCallingIdentity(PID, UID);
        IBinder secondCapability = DesktopRootProtocol.register(fixture.roots,
                fixture.rootIncarnation + 1, secondLifetime);
        Callback callbackB = fixture.attach(secondCapability, fixture.rootIncarnation + 1);
        Binder.setCallingIdentity(PID, UID);
        DesktopRootProtocol.bindWindow(secondCapability, secondOriginal.getToken(), 0);
        fixture.factOn(secondCapability, DesktopRootRegistry.ACTIVATED,
                fixture.rootIncarnation + 1, 1, true);
        pump();
        check(last(callbackA).originalChannelToken == null,
                "displaced root receives revocation");
        check(last(callbackB).originalChannelToken == secondOriginal.getToken(),
                "new root receives exact channel grant");
    }

    private static void testZeroWindowCloseRevokes() throws Exception {
        Fixture fixture = new Fixture();
        Callback callback = fixture.attachBeforeBind();
        fixture.bind();
        fixture.fact(DesktopRootRegistry.ACTIVATED, fixture.rootIncarnation, 1, true);
        pump();
        int before = callback.seen.size();
        fixture.removeWindow(fixture.window);
        pump();
        check(callback.seen.size() == before + 1, "zero-window transition publishes revoke");
        DesktopRootFocusDecision revoke = last(callback);
        check(revoke.originalChannelToken == null && revoke.factSerial == 1
                        && revoke.epoch > 0,
                "zero-window revoke retains fact serial and epoch while clearing channel");
        fixture.fact(DesktopRootRegistry.CLOSED, fixture.rootIncarnation, 2, false);
        pump();
        check(last(callback).originalChannelToken == null && last(callback).factSerial == 2
                && last(callback).sequence > revoke.sequence,
                "closed zero-window root delivers exact terminal revocation");
        check(((java.util.Map<?, ?>) field(field(fixture.publications, "rootDecisions"),
                "subscriptions")).isEmpty(), "ACKed terminal subscription was not released");
    }

    private static void testDeathDropsSubscription() throws Exception {
        Fixture fixture = new Fixture();
        Callback callback = fixture.attachBeforeBind();
        fixture.bind();
        fixture.fact(DesktopRootRegistry.ACTIVATED, fixture.rootIncarnation, 1, true);
        // Seal root admission before the queued grant is dispatched.  No callback ACK is
        // possible after death, and the exact subscription must disappear instead.
        int before = callback.seen.size();
        fixture.lifetime.die();
        pump();
        check(callback.seen.size() == before, "dead root drops queued subscription");
        check(((java.util.Map<?, ?>) field(field(fixture.publications, "rootDecisions"),
                "subscriptions")).isEmpty(), "dead root retained an impossible ACK obligation");
    }

    private static void testReentryAndOlderAckDoesNotClearRevoke() throws Exception {
        Fixture fixture = new Fixture();
        Callback callback = fixture.attachBeforeBind();
        callback.removeOnGrant = true;
        fixture.bind();
        fixture.fact(DesktopRootRegistry.ACTIVATED, fixture.rootIncarnation, 1, true);
        pump();
        check(callback.checkedOutsideLock, "decision callback reentered outside controller lock");
        check(callback.seen.size() >= 3, "reentry retained grant and newer revoke");
        DesktopRootFocusDecision grant = callback.seen.get(callback.seen.size() - 2);
        DesktopRootFocusDecision revoke = last(callback);
        check(grant.originalChannelToken != null && revoke.originalChannelToken == null,
                "older accepted grant cannot clear newer revocation");
        check(revoke.sequence > grant.sequence, "new revoke has a newer exact sequence");
    }

    private static void testRemoteRetryExactSequence() throws Exception {
        Fixture fixture = new Fixture();
        Callback callback = fixture.attachBeforeBind();
        pump(); // settle the initial epoch-zero revoke before injecting transport loss
        fixture.bind();
        callback.remoteFailure = true;
        fixture.fact(DesktopRootRegistry.ACTIVATED, fixture.rootIncarnation, 1, true);
        // Advance until the first callback attempt, which may follow WMS publication work.
        for (int i = 0; i < 32 && callback.attempted.isEmpty(); ++i)
            check(HandlerThread.runAnyNext(), "first callback retry task queued");
        callback.remoteFailure = false;
        pump();
        check(callback.attempted.size() == 1 && !callback.seen.isEmpty(),
                "generic RemoteException was retried");
        check(last(callback).sequence == callback.attempted.get(0).sequence,
                "successful ACK settles the retried exact sequence seen=" + callback.seen
                        + " attempted=" + callback.attempted);
    }

    private static void testCallbackFailureFailstop() throws Exception {
        Fixture fixture = new Fixture();
        Callback callback = fixture.attachBeforeBind();
        callback.runtimeFailure = true;
        fixture.bind();
        fixture.fact(DesktopRootRegistry.ACTIVATED, fixture.rootIncarnation, 1, true);
        pump();
        // A callback-side RuntimeException belongs to this exact subscription. It must retain
        // the decision for retry without poisoning unrelated WMS publication work.
        fixture.relayout(fixture.window); // While the callback is still rejecting every retry.
        callback.runtimeFailure = false;
        pump();
        check(callback.seen.size() >= 2, "callback runtime failure retained retry");
        fixture.relayout(fixture.window);
        check(true, "unrelated WMS publication remained healthy");
    }

    private static void testSnapshotSchedulingFailureIsFailstop() throws Exception {
        Fixture fixture = new Fixture();
        Callback callback = fixture.attachBeforeBind();
        pump();
        fixture.bind();
        // Let the WMS policy turn itself queue, then reject the delivery post created by
        // captureAll.  The committed snapshot must leave the driver unhealthy rather than
        // silently dropping the decision.
        Handler.allowPosts = 1;
        fixture.fact(DesktopRootRegistry.ACTIVATED, fixture.rootIncarnation, 1, true);
        pump();
        Handler.allowPosts = -1;
        rejected(() -> {
            try { fixture.relayout(fixture.window); }
            catch (Exception failure) { throw new IllegalStateException(failure); }
        }, "snapshot scheduling failure did not fail-stop WMS driver");
    }

    public static void main(String[] args) throws Exception {
        WindowInputPublisher.reset();
        testAttachBeforeBindAndNullEpoch();
        testBindActivationGrantAndOriginalToken();
        testIdenticalAttachAndRejects();
        testDisplacementRevokeAndGain();
        testZeroWindowCloseRevokes();
        testDeathDropsSubscription();
        testReentryAndOlderAckDoesNotClearRevoke();
        testRemoteRetryExactSequence();
        testCallbackFailureFailstop();
        testSnapshotSchedulingFailureIsFailstop();
        System.out.println("actual WMS/root focus decision checks=" + checks);
    }
}
