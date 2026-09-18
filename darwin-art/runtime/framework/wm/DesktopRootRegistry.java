package dev.darwinart.runtime.wm;

import android.os.IBinder;
import android.os.RemoteException;
import java.util.ArrayList;
import java.util.List;

/**
 * System-process owner of authenticated desktop-root registrations.
 *
 * <p>The registry deliberately owns only root authentication and the latest
 * accepted host fact.  It does not activate WMS focus or publish a focus
 * decision; those remain Android/WMS policy.</p>
 */
public final class DesktopRootRegistry {
    public static final int ACTIVATED = 1;
    public static final int RESIGNED = 2;
    public static final int CLOSED = 3;

    /** Definite pre-commit policy rejection; the caller may safely discard it. */
    static final class BindingRejected extends SecurityException {
        BindingRejected(String message) { super(message); }
        BindingRejected(String message, Throwable cause) {
            super(message);
            initCause(cause);
        }
    }

    static final class Fact {
        final int kind;
        final long incarnation;
        final long serial;
        final boolean keySnapshot;

        Fact(int eventKind, long rootIncarnation, long eventSerial, boolean snapshot) {
            kind = eventKind;
            incarnation = rootIncarnation;
            serial = eventSerial;
            keySnapshot = snapshot;
        }
    }

    /** Exact registration object used by death and stale-capability cleanup. */
    static final class Registration {
        final WindowSessionIdentity identity;
        final long incarnation;
        /* The registration itself is the stable opaque root identity. */
        final Object rootToken;
        final IBinder clientLifetime;
        final IBinder.DeathRecipient deathRecipient;
        final DesktopForegroundAuthority.ProcessInstance hostProcess;
        volatile Fact latestFact;
        final DesktopForegroundAuthority.Provider foreground;
        volatile boolean terminal;
        volatile boolean published;
        volatile boolean clientDied;
        boolean deathLinked;
        boolean deathUnlinked;

        Registration(WindowSessionIdentity owner, long rootIncarnation, IBinder lifetime,
                IBinder.DeathRecipient death, DesktopForegroundAuthority.ProcessInstance process,
                DesktopForegroundAuthority.Provider authority) {
            identity = owner;
            incarnation = rootIncarnation;
            rootToken = this;
            clientLifetime = lifetime;
            deathRecipient = death;
            hostProcess = process;
            foreground = authority;
        }

        boolean bindingOpen() { return !terminal && published; }
    }

    private static final class RetiredRoot {
        final WindowSessionIdentity identity;
        final long incarnation;

        RetiredRoot(WindowSessionIdentity owner, long rootIncarnation) {
            identity = owner;
            incarnation = rootIncarnation;
        }
    }

    /* Incarnations are scoped to an authenticated attachment, not global. */
    private final List<Registration> registrations = new ArrayList<>();
    /* Closed/dead roots cannot be reopened by the same attachment. */
    private final List<RetiredRoot> retiredRoots = new ArrayList<>();
    private final DesktopRootWindowBindingOwner bindingOwner;
    private final DesktopForegroundAuthority.Provider foreground;

    /** Authentication-only registry retained for tests and non-WMS callers. */
    public DesktopRootRegistry() { this(null, null); }

    /** Actual system-service registry with the shared WMS binding owner. */
    DesktopRootRegistry(DesktopRootWindowBindingOwner owner) {
        this(owner, DesktopForegroundAuthority.HOST);
    }

    DesktopRootRegistry(DesktopRootWindowBindingOwner owner,
            DesktopForegroundAuthority.Provider authority) {
        bindingOwner = owner;
        foreground = authority;
        if (owner != null && authority == null)
            throw new IllegalArgumentException("WMS roots require a host process authority");
    }

    /** Creates an un-published registration after checking its immutable input. */
    Registration prepare(WindowSessionIdentity identity, long incarnation, IBinder clientLifetime) {
        if (identity == null || incarnation == 0L || clientLifetime == null) {
            throw new IllegalArgumentException("invalid desktop root registration");
        }
        synchronized (this) {
            if (isRetired(identity, incarnation)) {
                throw new IllegalStateException("desktop root incarnation is stale");
            }
        }
        final Registration[] holder = new Registration[1];
        IBinder.DeathRecipient death = () -> {
            holder[0].clientDied = true;
            remove(holder[0]);
        };
        // Pin the kernel process instance server-side, never trust client birth data.
        // Capture is outside this monitor and does not dispatch to AppKit's actor.
        DesktopForegroundAuthority.ProcessInstance process =
                foreground == null ? null : foreground.capture(identity.pid());
        if (foreground != null && (process == null || process.pid != identity.pid()))
            throw new SecurityException("host process capture does not match attachment");
        identity.requireCaller(identity.pid(), identity.uid());
        Registration registration = new Registration(identity, incarnation, clientLifetime, death,
                process, foreground);
        holder[0] = registration;
        return registration;
    }

    /**
     * Links the client lifetime before publishing the registration.
     *
     * <p>Binder implementations may report a death immediately from
     * {@code linkToDeath}.  The registration is therefore checked both before
     * and after linking; the synchronized publication also closes the race
     * where a death callback waits for this monitor.</p>
     */
    void linkAndPublish(Registration registration) throws RemoteException {
        if (registration == null) throw new IllegalArgumentException("registration is null");
        synchronized (this) {
            if (registration.terminal) throw new IllegalStateException("registration is terminal");
            if (isRetired(registration.identity, registration.incarnation)) {
                throw new IllegalStateException("desktop root incarnation is stale");
            }
            Registration existing = find(registration.identity, registration.incarnation);
            if (existing != null) {
                if (!existing.clientLifetime.equals(registration.clientLifetime)) {
                    throw new SecurityException("desktop root belongs to another client lifetime");
                }
                throw new IllegalStateException("desktop root already registered for attachment");
            }
            // Reserve the exact slot before leaving the monitor so concurrent
            // REGISTER calls cannot both link and publish the same root.
            registrations.add(registration);
        }

        try {
            // Binder may invoke the death recipient synchronously. This call
            // intentionally happens outside the registry monitor.
            registration.clientLifetime.linkToDeath(registration.deathRecipient, 0);
            synchronized (this) {
                registration.deathLinked = true;
                if (registration.terminal || !contains(registration)) {
                    throw new IllegalStateException("desktop root client died during registration");
                }
                // Recheck the pinned attachment after external Binder IO and
                // before making the capability visible to the caller.
                registration.identity.requireCaller(
                        registration.identity.pid(), registration.identity.uid());
                if (isRetired(registration.identity, registration.incarnation)) {
                    throw new IllegalStateException("desktop root retired during registration");
                }
                if (registration.terminal || !contains(registration)) {
                    throw new IllegalStateException("desktop root attachment retired during registration");
                }
                registration.published = true;
            }
        } catch (RuntimeException | RemoteException failure) {
            remove(registration);
            throw failure;
        }
    }

    /**
     * Records one retained host fact for the exact capability registration.
     * Returns true only after the fact is retained by this registry.
     */
    boolean recordFact(Registration registration, int kind,
            long incarnation, long serial, boolean keySnapshot) {
        Registration unlink = null;
        RuntimeException callerFailure = null;
        Fact retained = null;
        synchronized (this) {
            if (registration == null || !contains(registration) || registration.terminal
                    || !registration.published) {
                throw new SecurityException("stale desktop root capability");
            }
            // A foreign holder must not be able to revoke the legitimate
            // owner's capability. Only its own stale attachment is retired.
            if (android.os.Binder.getCallingPid() != registration.identity.pid()
                    || android.os.Binder.getCallingUid() != registration.identity.uid()) {
                throw new SecurityException("desktop root caller identity mismatch");
            }
            try {
                registration.identity.requireCaller(
                        android.os.Binder.getCallingPid(), android.os.Binder.getCallingUid());
            } catch (RuntimeException failure) {
                unlink = detachLocked(registration);
                callerFailure = failure;
            }
            if (callerFailure != null) {
                // The exact registration is removed below the monitor.
            } else {
                validateFact(kind, incarnation, serial, keySnapshot);
                if (registration.incarnation != incarnation) {
                    throw new SecurityException("desktop root incarnation mismatch");
                }
                Fact previous = registration.latestFact;
                if (previous != null
                        && Long.compareUnsigned(serial, previous.serial) <= 0) {
                    throw new IllegalStateException("desktop root fact serial is stale");
                }
                // The immutable fact becomes visible before acknowledging the call.
                registration.latestFact = new Fact(kind, incarnation, serial, keySnapshot);
                retained = registration.latestFact;
                if (kind == CLOSED) unlink = detachLocked(registration);
            }
        }
        if (unlink != null) cleanupDetached(registration, unlink);
        if (callerFailure != null) throw callerFailure;
        if (bindingOwner != null && retained != null && kind != CLOSED)
            bindingOwner.factChanged(registration, retained);
        return true;
    }

    /**
     * Binds an original WMS server-channel token to this exact root.  The
     * authentication snapshot is pinned under this monitor; the binding owner
     * is deliberately called outside it, then the root is revalidated before
     * the bind is accepted.
     */
    boolean bindWindow(Registration registration, IBinder originalChannelToken, int displayId) {
        if (originalChannelToken == null)
            throw new BindingRejected("channel token is null");
        if (displayId != 0) throw new BindingRejected("unknown logical display");
        return executeBinding(registration,
                () -> bindingOwner.bind(registration, originalChannelToken, displayId));
    }

    boolean attachFocusDecisions(Registration registration, IBinder callback) {
        if (callback == null) throw new BindingRejected("focus callback is null");
        return executeBinding(registration,
                () -> bindingOwner.attachFocusDecisions(registration, callback));
    }

    /** Exact capability admission shared by window bind and typed decision attach. */
    private boolean executeBinding(Registration registration, Runnable operation) {
        DesktopRootWindowBindingOwner owner;
        Registration preBindDetached = null;
        RuntimeException preBindFailure = null;
        synchronized (this) {
            if (!isCurrentPublishedLocked(registration))
                throw new BindingRejected("stale desktop root capability");
            int pid = android.os.Binder.getCallingPid();
            int uid = android.os.Binder.getCallingUid();
            if (pid != registration.identity.pid() || uid != registration.identity.uid())
                throw new BindingRejected("desktop root caller identity mismatch");
            try {
                registration.identity.requireCaller(pid, uid);
            } catch (RuntimeException failure) {
                preBindDetached = detachLocked(registration);
                preBindFailure = failure;
            }
            owner = bindingOwner;
            if (owner == null) {
                throw new UnsupportedOperationException("desktop root window binding unavailable");
            }
        }
        if (preBindFailure != null) {
            cleanupDetached(registration, preBindDetached);
            throw new BindingRejected("desktop root attachment is stale", preBindFailure);
        }
        operation.run();
        boolean rollback;
        Registration detached = null;
        synchronized (this) {
            rollback = !isCurrentPublishedLocked(registration);
            if (!rollback) {
                try {
                    // The attachment snapshot is pinned again after the
                    // external WMS call; a recycled PID/UID must not retain
                    // the bind merely because the old death callback lagged.
                    requireCurrentCallerLocked(registration);
                } catch (RuntimeException stale) {
                    rollback = true;
                }
            }
            if (rollback && registration != null && !registration.terminal
                    && contains(registration)) {
                // Seal admission before any outside-monitor cleanup.  This
                // prevents another bind from being admitted between the
                // stale check and rollback.
                detached = detachLocked(registration);
            }
        }
        if (rollback) {
            // A concurrent CLOSED/death seals binding admission before its
            // outside-monitor unbind.  Roll back this exact root; owner
            // unbind is idempotent and serialized with close.
            RuntimeException cleanupFailure = null;
            if (detached != null) {
                try {
                    cleanupDetached(registration, detached);
                } catch (RuntimeException failure) {
                    cleanupFailure = failure;
                }
            }
            try {
                owner.unbind(registration);
            } catch (RuntimeException failure) {
                if (cleanupFailure == null) cleanupFailure = failure;
                else cleanupFailure.addSuppressed(failure);
            }
            if (cleanupFailure != null) throw cleanupFailure;
            throw new BindingRejected("desktop root closed during bind");
        }
        return true;
    }

    /** Removes only the exact registration object, never an arbitrary PID. */
    void remove(Registration registration) {
        Registration unlink;
        synchronized (this) {
            unlink = detachLocked(registration);
        }
        cleanupDetached(registration, unlink);
    }

    private Registration detachLocked(Registration registration) {
        if (registration == null) return null;
        boolean wasPublished = registration.published;
        registration.terminal = true;
        registrations.remove(registration);
        if (wasPublished) {
            rememberRetiredLocked(registration.identity, registration.incarnation);
        }
        if (registration.deathLinked && !registration.deathUnlinked) {
            registration.deathUnlinked = true;
            return registration;
        }
        return null;
    }

    private void requireCurrentCallerLocked(Registration registration) {
        if (!isCurrentPublishedLocked(registration))
            throw new SecurityException("stale desktop root capability");
        int pid = android.os.Binder.getCallingPid();
        int uid = android.os.Binder.getCallingUid();
        if (pid != registration.identity.pid() || uid != registration.identity.uid())
            throw new SecurityException("desktop root caller identity mismatch");
        registration.identity.requireCaller(pid, uid);
    }

    private boolean isCurrentPublishedLocked(Registration registration) {
        return registration != null && !registration.terminal && registration.published
                && contains(registration);
    }

    /** Runs exact-root unbind and Binder unlink only after leaving the monitor. */
    private void cleanupDetached(Registration registration, Registration unlink) {
        if (registration == null) return;
        RuntimeException failure = null;
        if (bindingOwner != null) {
            try {
                bindingOwner.unbind(registration);
            } catch (RuntimeException error) {
                failure = error;
            }
        }
        try {
            if (unlink != null) unlinkDeath(unlink);
        } catch (RuntimeException error) {
            if (failure == null) failure = error;
            else failure.addSuppressed(error);
        }
        if (failure != null) throw failure;
    }

    private static void unlinkDeath(Registration registration) {
        try {
            registration.clientLifetime.unlinkToDeath(registration.deathRecipient, 0);
        } catch (RuntimeException ignored) {
            // The registration is already terminal; Binder cleanup is best effort.
        }
    }

    // Package-private test/owner observation; it never exposes mutable state.
    synchronized Fact latestFact(Registration registration) {
        if (registration == null || !contains(registration)) return null;
        return registration.latestFact;
    }

    synchronized int registrationCount() {
        return registrations.size();
    }

    private boolean isRetired(WindowSessionIdentity identity, long incarnation) {
        for (RetiredRoot retired : retiredRoots) {
            if (retired.identity.sameAttachment(identity)
                    && Long.compareUnsigned(incarnation, retired.incarnation) <= 0) return true;
        }
        return false;
    }

    private void rememberRetiredLocked(WindowSessionIdentity identity, long incarnation) {
        for (RetiredRoot retired : retiredRoots) {
            if (!retired.identity.sameAttachment(identity)) continue;
            if (Long.compareUnsigned(incarnation, retired.incarnation) > 0) {
                retiredRoots.remove(retired);
                break;
            }
            return;
        }
        retiredRoots.add(new RetiredRoot(identity, incarnation));
    }

    private Registration find(WindowSessionIdentity identity, long incarnation) {
        for (Registration candidate : registrations) {
            if (candidate.incarnation == incarnation
                    && candidate.identity.sameAttachment(identity)) return candidate;
        }
        return null;
    }

    private boolean contains(Registration registration) {
        for (Registration candidate : registrations) if (candidate == registration) return true;
        return false;
    }

    private static void validateFact(int kind, long incarnation, long serial,
            boolean keySnapshot) {
        if (incarnation == 0L || serial == 0L) {
            throw new IllegalArgumentException("desktop root identity is zero");
        }
        if (kind == ACTIVATED) {
            if (!keySnapshot) throw new IllegalArgumentException("activated root is not key");
        } else if (kind == RESIGNED) {
            if (keySnapshot) throw new IllegalArgumentException("resigned root is key");
        } else if (kind != CLOSED) {
            throw new IllegalArgumentException("unknown desktop root fact kind");
        }
    }
}
