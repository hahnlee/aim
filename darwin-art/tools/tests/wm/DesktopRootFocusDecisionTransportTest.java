package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;

/** Actual production wire/receiver objects with deterministic Binder/Parcel fixture ports. */
public final class DesktopRootFocusDecisionTransportTest {
    private static final int SERVER_PID = 7001;
    private static final int SERVER_UID = 17001;
    private static final int FOREIGN_PID = 7002;
    private static final int FOREIGN_UID = 17002;

    private static final class Token extends Binder {}

    private static final class Receiver extends DesktopRootFocusDecisionTransport.Receiver {
        int calls;
        DesktopRootFocusDecision last;
        boolean reenter;
        @Override protected void onDecision(DesktopRootFocusDecision decision) {
            ++calls;
            last = decision;
            if (reenter && decision.sequence == 3L) {
                reenter = false;
                send(decision(4L, 11L, 4L));
            }
        }
        private void send(DesktopRootFocusDecision decision) {
            try {
                check(DesktopRootFocusDecisionTransport.send(this, decision),
                        "reentrant decision rejected");
            } catch (RemoteException failure) {
                throw new AssertionError(failure);
            }
        }
    }

    private static DesktopRootFocusDecision decision(long sequence, long factSerial,
            long epoch) {
        return new DesktopRootFocusDecision(11L, factSerial, sequence, new Token(), epoch);
    }

    private static DesktopRootFocusDecision revoke(long sequence, long factSerial, long epoch) {
        return new DesktopRootFocusDecision(11L, factSerial, sequence, null, epoch);
    }

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static void expect(Class<? extends Throwable> type, Throwing action, String message) {
        try {
            action.run();
        } catch (Throwable failure) {
            if (type.isInstance(failure)) return;
            throw new AssertionError(message + ": wrong exception " + failure, failure);
        }
        throw new AssertionError(message);
    }

    private interface Throwing { void run() throws Exception; }

    private static boolean send(Receiver receiver, DesktopRootFocusDecision decision)
            throws Exception {
        return DesktopRootFocusDecisionTransport.send(receiver, decision);
    }

    private static Parcel wire(int version, DesktopRootFocusDecision decision) {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(DesktopRootFocusDecisionTransport.DESCRIPTOR);
        data.writeInt(version);
        data.writeLong(decision.incarnation);
        data.writeLong(decision.factSerial);
        data.writeLong(decision.sequence);
        data.writeStrongBinder(decision.originalChannelToken);
        data.writeLong(decision.epoch);
        return data;
    }

    private static void testValidationAndStrictWire() throws Exception {
        expect(IllegalArgumentException.class,
                () -> new DesktopRootFocusDecision(0L, 1L, 1L, new Token(), 1L),
                "zero incarnation accepted");
        expect(IllegalArgumentException.class,
                () -> new DesktopRootFocusDecision(1L, 0L, 1L, new Token(), 1L),
                "zero grant fact accepted");
        expect(IllegalArgumentException.class,
                () -> new DesktopRootFocusDecision(1L, 1L, 1L, new Token(), 0L),
                "zero grant epoch accepted");

        Receiver receiver = new Receiver();
        Binder.setCallingIdentity(SERVER_PID, SERVER_UID);
        Parcel wrongVersion = wire(2, decision(1L, 1L, 1L));
        expect(IllegalArgumentException.class,
                () -> receiver.transact(DesktopRootFocusDecisionTransport.TRANSACTION_DECISION,
                        wrongVersion, Parcel.obtain(), 0), "wrong version accepted");
        Parcel trailing = wire(1, decision(1L, 1L, 1L));
        trailing.writeInt(9);
        expect(IllegalStateException.class,
                () -> receiver.transact(DesktopRootFocusDecisionTransport.TRANSACTION_DECISION,
                        trailing, Parcel.obtain(), 0), "trailing data accepted");
    }

    private static void testHandshakeInboxAuthAndReentry() throws Exception {
        Receiver receiver = new Receiver();
        Binder.setCallingIdentity(FOREIGN_PID, FOREIGN_UID);
        check(send(receiver, decision(1L, 1L, 1L)), "early decision not bounded-enqueued");
        Binder.setCallingIdentity(SERVER_PID, SERVER_UID);
        check(send(receiver, revoke(2L, 9L, 2L)), "second early decision not enqueued");
        check(receiver.calls == 0, "early decision delivered before pin");
        check(receiver.pinServer(SERVER_PID, SERVER_UID), "trusted pin rejected");
        check(receiver.calls == 1 && receiver.last.originalChannelToken == null,
                "matching early revoke was not delivered");
        check(!receiver.pinServer(FOREIGN_PID, FOREIGN_UID), "wrong repin accepted");
        Binder.setCallingIdentity(FOREIGN_PID, FOREIGN_UID);
        check(!send(receiver, decision(3L, 10L, 3L)), "foreign callback accepted");

        Binder.setCallingIdentity(SERVER_PID, SERVER_UID);
        receiver.reenter = true;
        check(send(receiver, decision(3L, 10L, 3L)), "same-serial grant rejected");
        check(receiver.calls == 3, "callback reentry/deadlock or delivery failure");
        int calls = receiver.calls;
        check(send(receiver, receiver.last), "exact duplicate retry rejected");
        check(receiver.calls == calls + 1, "exact retry did not re-run owner retention");
        expect(IllegalArgumentException.class,
                () -> send(receiver, new DesktopRootFocusDecision(11L, 11L, 4L,
                        new Token(), 44L)), "conflicting duplicate accepted");
        check(!send(receiver, decision(2L, 11L, 2L)), "older sequence accepted");
        check(!send(receiver, decision(5L, 1L, 5L)),
                "unsigned fact-serial regression accepted");
        check(send(receiver, revoke(5L, 0L, 5L)), "zero-serial revoke rejected");
    }

    private static void testReplyFailure() throws Exception {
        final DesktopRootFocusDecision value = decision(1L, 1L, 1L);
        IBinder returnsFalse = new Binder() {
            @Override public boolean transact(int code, Parcel data, Parcel reply, int flags) {
                return false;
            }
        };
        expect(RemoteException.class,
                () -> DesktopRootFocusDecisionTransport.send(returnsFalse, value),
                "false transact was not uncertain");
        IBinder throwsRemote = new Binder() {
            @Override public boolean transact(int code, Parcel data, Parcel reply, int flags)
                    throws RemoteException {
                throw new RemoteException("broken reply");
            }
        };
        expect(RemoteException.class,
                () -> DesktopRootFocusDecisionTransport.send(throwsRemote, value),
                "remote reply failure was swallowed");
    }

    private static int retainedSize(Receiver receiver, String field) throws Exception {
        java.lang.reflect.Field member = DesktopRootFocusDecisionTransport.Receiver.class
                .getDeclaredField(field);
        member.setAccessible(true);
        Object retained = member.get(receiver);
        return retained instanceof java.util.Map ? ((java.util.Map<?, ?>) retained).size()
                : ((java.util.Collection<?>) retained).size();
    }

    private static void testTerminalTransportClosure() throws Exception {
        Binder.setCallingIdentity(SERVER_PID, SERVER_UID);
        Receiver early = new Receiver();
        check(send(early, decision(1, 1, 1)) && retainedSize(early, "early") == 1,
                "early token record was not retained");
        early.closeTransportAdmission();
        check(retainedSize(early, "early") == 0 && retainedSize(early, "highwater") == 0,
                "terminal close retained pre-handshake token records");
        check(!early.pinServer(SERVER_PID, SERVER_UID) && !send(early, decision(2, 2, 2)),
                "terminal close permitted late pin/callback refill");
        Receiver pinned = new Receiver();
        check(pinned.pinServer(SERVER_PID, SERVER_UID) && send(pinned, decision(1, 1, 1)),
                "pinned grant not retained");
        check(retainedSize(pinned, "highwater") == 1, "pinned token watermark missing");
        pinned.closeTransportAdmission();
        check(!pinned.isServerPinned() && retainedSize(pinned, "highwater") == 0
                && !send(pinned, revoke(2, 2, 2)), "closed watermark refilled after shutdown");
    }

    public static void main(String[] args) throws Exception {
        testValidationAndStrictWire();
        testHandshakeInboxAuthAndReentry();
        testReplyFailure();
        testTerminalTransportClosure();
        System.out.println("Desktop-root focus decision wire/auth/order/reentry: PASS");
    }
}
