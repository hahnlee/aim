package dev.darwinart.runtime.am;

import android.os.IBinder;
import android.os.RemoteException;
import java.util.NoSuchElementException;

/** Owns one exact Binder death-registration attempt and its resource tail. */
final class ServiceConnectionDeathRegistration implements IBinder.DeathRecipient {
    interface Callback {
        void binderDied(ServiceConnectionDeathRegistration registration);
    }

    interface PresenceQuery {
        int query(IBinder binder, IBinder.DeathRecipient recipient);
    }

    static final int UNSUPPORTED = -1;
    static final int ABSENT = 0;
    static final int PRESENT = 1;

    enum State { LINKING, LINKED, DETACHED, UNLINK_CLAIMED }

    final IBinder binder;
    private final Callback callback;
    private final PresenceQuery presenceQuery;
    private volatile boolean linkInvocationComplete;
    private boolean linkInvoked;
    private boolean unlinkInFlight;
    State state = State.LINKING;
    boolean dead;

    ServiceConnectionDeathRegistration(IBinder connectionBinder, Callback deathCallback) {
        this(connectionBinder, deathCallback,
                ServiceConnectionDeathRegistration::nativeQueryRecipientPresence);
    }

    ServiceConnectionDeathRegistration(IBinder connectionBinder, Callback deathCallback,
            PresenceQuery resourcePresenceQuery) {
        if (connectionBinder == null || deathCallback == null || resourcePresenceQuery == null) {
            throw new IllegalArgumentException("Missing death registration owner");
        }
        binder = connectionBinder;
        callback = deathCallback;
        presenceQuery = resourcePresenceQuery;
    }

    /** Must be called without the ActiveServices monitor. */
    void linkOutsideLock() throws RemoteException {
        synchronized (this) {
            if (linkInvoked) throw new IllegalStateException("Death registration cannot relink");
            linkInvoked = true;
        }
        try {
            binder.linkToDeath(this, 0);
        } finally {
            linkInvocationComplete = true;
        }
    }

    /** Caller owns the ActiveServices monitor. Returns whether unlink is needed. */
    boolean finishLinkLocked() {
        if (dead && state == State.LINKING) {
            state = State.UNLINK_CLAIMED;
            return true;
        }
        if (state == State.LINKING) {
            state = State.LINKED;
            return false;
        }
        if (state == State.DETACHED) {
            state = State.UNLINK_CLAIMED;
            return true;
        }
        return false;
    }

    /** Caller owns the ActiveServices monitor. */
    boolean failLinkLocked(boolean ambiguousInstallation) {
        if (ambiguousInstallation && (state == State.LINKING || state == State.DETACHED)) {
            state = State.UNLINK_CLAIMED;
            return true;
        }
        if (state == State.LINKING) {
            state = State.DETACHED;
            return false;
        }
        if (state == State.LINKED) {
            state = State.UNLINK_CLAIMED;
            return true;
        }
        return false;
    }

    /** Caller owns the ActiveServices monitor. */
    boolean claimUnlinkLocked() {
        if (state == State.LINKED) {
            state = State.UNLINK_CLAIMED;
            return true;
        }
        if (state == State.LINKING) state = State.DETACHED;
        return false;
    }

    /** Caller owns the ActiveServices monitor. */
    boolean isLinkedLocked() { return state == State.LINKED && !dead; }

    /** Must be called without the ActiveServices monitor. */
    void unlinkOutsideLock() {
        synchronized (this) {
            if (!linkInvocationComplete || unlinkInFlight) {
                throw new IllegalStateException("Unowned or concurrent death-registration unlink");
            }
            unlinkInFlight = true;
        }
        try {
            // Preserve public BinderProxy cleanup, including its Java recipient list.
            binder.unlinkToDeath(this, 0);
        } catch (NoSuchElementException original) {
            try {
                if (presenceQuery.query(binder, this) == ABSENT) return;
            } catch (RuntimeException | Error queryFailure) {
                if (queryFailure != original) original.addSuppressed(queryFailure);
            }
            throw original;
        } finally {
            synchronized (this) { unlinkInFlight = false; }
        }
    }

    private static native int nativeQueryRecipientPresence(
            IBinder binder, IBinder.DeathRecipient recipient);

    @Override
    public void binderDied() {
        callback.binderDied(this);
    }
}

/** Exact pending bind pin held until the link attempt is admitted or rejected. */
final class PendingServiceConnectionBind {
    final String serviceKey;
    final ServiceRecord service;
    final IntentBindRecord binding;
    final ConnectionRecord connection;
    final IBinder.DeathRecipient expectedDeath;
    final ServiceConnectionDeathRegistration registration;

    PendingServiceConnectionBind(String key, ServiceRecord owner, IntentBindRecord intentBinding,
            ConnectionRecord record, IBinder.DeathRecipient death,
            ServiceConnectionDeathRegistration deathRegistration) {
        serviceKey = key;
        service = owner;
        binding = intentBinding;
        connection = record;
        expectedDeath = death;
        registration = deathRegistration;
    }
}
