package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.Map;

/** Versioned, authenticated Binder transport for desktop-root focus decisions. */
public final class DesktopRootFocusDecisionTransport {
    public static final String DESCRIPTOR =
            "dev.darwinart.runtime.wm.IDesktopRootFocusDecisions";
    public static final int TRANSACTION_DECISION = IBinder.FIRST_CALL_TRANSACTION;
    /** Alias used by callers that name the operation after its wire verb. */
    public static final int TRANSACTION_SEND = TRANSACTION_DECISION;
    public static final int VERSION = 1;
    public static final int VERSION_1 = VERSION;
    /** The receiver never allocates an unbounded queue before its handshake. */
    public static final int MAX_EARLY_INBOX = 64;

    private DesktopRootFocusDecisionTransport() {}

    /** Synchronously sends one exact decision and returns the receiver's acceptance bit. */
    public static boolean send(IBinder callback, DesktopRootFocusDecision decision)
            throws RemoteException {
        if (callback == null) throw new IllegalArgumentException("callback is null");
        if (decision == null) throw new IllegalArgumentException("decision is null");
        Parcel data = Parcel.obtain();
        Parcel reply = Parcel.obtain();
        try {
            data.writeInterfaceToken(DESCRIPTOR);
            data.writeInt(VERSION);
            data.writeLong(decision.incarnation);
            data.writeLong(decision.factSerial);
            data.writeLong(decision.sequence);
            data.writeStrongBinder(decision.originalChannelToken);
            data.writeLong(decision.epoch);
            if (!callback.transact(TRANSACTION_DECISION, data, reply, 0)) {
                throw new RemoteException("desktop-root focus decision rejected");
            }
            reply.readException();
            return reply.readBoolean();
        } finally {
            reply.recycle();
            data.recycle();
        }
    }

    /**
     * Typed callback endpoint. The server identity must be pinned through a
     * trusted handshake before callback records are treated as authoritative.
     */
    public abstract static class Receiver extends Binder {
        private final Object lock = new Object();
        private final ArrayDeque<Pending> early = new ArrayDeque<>();
        private final Map<Long, Watermark> highwater = new HashMap<>();
        private boolean serverPinned;
        private int serverPid;
        private int serverUid;
        private boolean admissionClosed;

        protected Receiver() {
            attachInterface(null, DESCRIPTOR);
        }

        /**
         * Pins the trusted WMS server identity and drains matching early
         * records. A same-identity pin is idempotent; a different repin fails.
         */
        public final boolean pinServer(int pid, int uid) {
            if (pid <= 0 || uid < 0) throw new IllegalArgumentException("invalid server identity");
            ArrayList<DesktopRootFocusDecision> deliver = new ArrayList<>();
            synchronized (lock) {
                if (admissionClosed) return false;
                if (serverPinned) {
                    if (serverPid != pid || serverUid != uid) return false;
                } else {
                    serverPinned = true;
                    serverPid = pid;
                    serverUid = uid;
                }
                while (!early.isEmpty()) {
                    Pending pending = early.removeFirst();
                    if (pending.pid == serverPid && pending.uid == serverUid
                            && admitLocked(pending.decision, deliver)) {
                        // admitLocked selected a fresh decision for delivery.
                    }
                }
            }
            deliverOutsideMonitor(deliver);
            return true;
        }

        /** Returns whether the explicit handshake has established the server identity. */
        public final boolean isServerPinned() {
            synchronized (lock) { return !admissionClosed && serverPinned; }
        }

        /** Terminal transport shutdown; late callbacks cannot refill token retention. */
        public final void closeTransportAdmission() {
            synchronized (lock) {
                admissionClosed = true;
                early.clear();
                highwater.clear();
            }
        }

        /** Returns the callback payload only after identity and ordering admission. */
        protected abstract void onDecision(DesktopRootFocusDecision decision);

        @Override
        protected final boolean onTransact(int code, Parcel data, Parcel reply, int flags)
                throws RemoteException {
            if (code != TRANSACTION_DECISION) return super.onTransact(code, data, reply, flags);
            if (data == null || reply == null || (flags & IBinder.FLAG_ONEWAY) != 0) return false;
            data.enforceInterface(DESCRIPTOR);
            int version = data.readInt();
            if (version != VERSION) throw new IllegalArgumentException("unsupported version");
            long incarnation = data.readLong();
            long factSerial = data.readLong();
            long sequence = data.readLong();
            IBinder originalChannelToken = data.readStrongBinder();
            long epoch = data.readLong();
            data.enforceNoDataAvail();
            DesktopRootFocusDecision decision = new DesktopRootFocusDecision(
                    incarnation, factSerial, sequence, originalChannelToken, epoch);

            final ArrayList<DesktopRootFocusDecision> deliver = new ArrayList<>(1);
            final boolean accepted;
            final int callerPid = Binder.getCallingPid();
            final int callerUid = Binder.getCallingUid();
            synchronized (lock) {
                if (admissionClosed) {
                    accepted = false;
                } else if (!serverPinned) {
                    if (early.size() >= MAX_EARLY_INBOX) {
                        accepted = false;
                    } else {
                        early.addLast(new Pending(callerPid, callerUid, decision));
                        accepted = true;
                    }
                } else if (callerPid != serverPid || callerUid != serverUid) {
                    accepted = false;
                } else {
                    accepted = admitLocked(decision, deliver);
                }
            }
            // Never invoke owner code while holding Receiver's admission lock.
            deliverOutsideMonitor(deliver);
            reply.writeNoException();
            reply.writeBoolean(accepted);
            return true;
        }

        private boolean admitLocked(DesktopRootFocusDecision decision,
                ArrayList<DesktopRootFocusDecision> deliver) {
            Watermark prior = highwater.get(Long.valueOf(decision.incarnation));
            if (prior != null) {
                if (decision.sequence == prior.decision.sequence) {
                    // Retransmission is idempotent only for the exact immutable value.
                    if (!decision.equals(prior.decision)) {
                        throw new IllegalArgumentException("sequence reused with different decision");
                    }
                    // Re-run retention for an exact retry. The owner is
                    // responsible for making this callback idempotent; this
                    // also permits retry after a callback-side failure.
                    deliver.add(decision);
                    return true;
                }
                if (decision.sequence < prior.decision.sequence) return false;
                // Revocation serial zero is a sequence-ordered marker, not a
                // numeric reset of the unsigned non-zero fact serial highwater.
                if (decision.factSerial != 0L && prior.nonzeroFactSerial != 0L
                        && Long.compareUnsigned(decision.factSerial, prior.nonzeroFactSerial) < 0) {
                    return false;
                }
            }
            long nonzeroFactSerial = decision.factSerial == 0L
                    ? (prior == null ? 0L : prior.nonzeroFactSerial) : decision.factSerial;
            highwater.put(Long.valueOf(decision.incarnation),
                    new Watermark(decision, nonzeroFactSerial));
            // A null original token is the explicit revoke decision and must
            // reach the owner just like a grant.
            deliver.add(decision);
            return true;
        }

        private void deliverOutsideMonitor(ArrayList<DesktopRootFocusDecision> deliver) {
            for (DesktopRootFocusDecision decision : deliver) onDecision(decision);
        }

        private static final class Pending {
            final int pid;
            final int uid;
            final DesktopRootFocusDecision decision;
            Pending(int pid, int uid, DesktopRootFocusDecision decision) {
                this.pid = pid;
                this.uid = uid;
                this.decision = decision;
            }
        }

        private static final class Watermark {
            final DesktopRootFocusDecision decision;
            final long nonzeroFactSerial;
            Watermark(DesktopRootFocusDecision decision, long nonzeroFactSerial) {
                this.decision = decision;
                this.nonzeroFactSerial = nonzeroFactSerial;
            }
        }
    }
}
