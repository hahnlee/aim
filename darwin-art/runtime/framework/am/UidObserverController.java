package dev.darwinart.runtime.am;

import android.app.ActivityManager;
import android.app.IUidObserver;
import android.os.Binder;
import android.os.IBinder;
import android.os.RemoteCallbackList;
import android.os.RemoteException;
import android.util.SparseIntArray;

/**
 * ActivityManagerService's UidObserverController: the uid state, capability,
 * active/gone and cached changes {@link UidProcessStates} computes, delivered
 * to registered IUidObservers according to each registration's flags,
 * cutpoint and optional uid filter. Idle transitions are never reported: a
 * uid here does not enter the background-idle (app standby) state.
 */
final class UidObserverController {
    private static final class Registration {
        final int which;
        final int cutpoint;
        // Null observes every uid (registerUidObserver).
        final android.util.ArraySet<Integer> uids;
        final IBinder token = new Binder();
        final SparseIntArray reportedStates = new SparseIntArray();

        Registration(int which, int cutpoint, int[] filter) {
            this.which = which;
            this.cutpoint = cutpoint;
            if (filter == null) {
                uids = null;
            } else {
                uids = new android.util.ArraySet<>();
                for (int uid : filter) uids.add(uid);
            }
        }

        boolean observes(int uid) {
            return uids == null || uids.contains(uid);
        }
    }

    private final RemoteCallbackList<IUidObserver> observers = new RemoteCallbackList<>();

    /** registerUidObserver / registerUidObserverForUids; returns the filter token. */
    IBinder register(IUidObserver observer, int which, int cutpoint, int[] uids) {
        if (observer == null) throw new IllegalArgumentException("observer is null");
        Registration registration = new Registration(which, cutpoint, uids);
        synchronized (observers) {
            observers.register(observer, registration);
        }
        return registration.token;
    }

    void unregister(IUidObserver observer) {
        if (observer == null) return;
        synchronized (observers) {
            observers.unregister(observer);
        }
    }

    /** addUidToObserver / removeUidFromObserver for a filtered registration. */
    void updateFilter(IBinder token, int uid, boolean add) {
        synchronized (observers) {
            for (int i = observers.getRegisteredCallbackCount() - 1; i >= 0; i--) {
                Registration registration = (Registration) observers.getRegisteredCallbackCookie(i);
                if (registration.token != token || registration.uids == null) continue;
                if (add) {
                    registration.uids.add(uid);
                } else {
                    registration.uids.remove(uid);
                }
                return;
            }
        }
        throw new IllegalArgumentException("unknown uid observer token");
    }

    /**
     * One uid's state changed from {@code previous} to {@code state}
     * (PROCESS_STATE_NONEXISTENT when the uid appeared or is gone).
     */
    void dispatch(int uid, int previous, int state, int previousCapability, int capability) {
        synchronized (observers) {
            int count = observers.beginBroadcast();
            try {
                for (int i = 0; i < count; i++) {
                    Registration registration = (Registration) observers.getBroadcastCookie(i);
                    if (!registration.observes(uid)) continue;
                    try {
                        deliver(observers.getBroadcastItem(i), registration, uid, previous, state,
                                previousCapability, capability);
                    } catch (RemoteException ignored) {
                        // A dead observer is dropped by the callback list.
                    }
                }
            } finally {
                observers.finishBroadcast();
            }
        }
    }

    private static void deliver(IUidObserver observer, Registration registration, int uid,
            int previous, int state, int previousCapability, int capability)
            throws RemoteException {
        final int which = registration.which;
        final boolean gone = state == ActivityManager.PROCESS_STATE_NONEXISTENT;
        final boolean appeared = previous == ActivityManager.PROCESS_STATE_NONEXISTENT;
        if (gone) {
            registration.reportedStates.delete(uid);
            if ((which & ActivityManager.UID_OBSERVER_GONE) != 0) {
                observer.onUidGone(uid, false);
            }
            return;
        }
        if (appeared && (which & ActivityManager.UID_OBSERVER_ACTIVE) != 0) {
            observer.onUidActive(uid);
        }
        if ((which & ActivityManager.UID_OBSERVER_CACHED) != 0) {
            boolean wasCached = !appeared
                    && previous >= ActivityManager.PROCESS_STATE_CACHED_ACTIVITY;
            boolean cached = state >= ActivityManager.PROCESS_STATE_CACHED_ACTIVITY;
            if (appeared ? cached : wasCached != cached) observer.onUidCachedChanged(uid, cached);
        }
        boolean stateReport = (which & ActivityManager.UID_OBSERVER_PROCSTATE) != 0;
        if (stateReport && registration.cutpoint >= ActivityManager.MIN_PROCESS_STATE) {
            // Only crossings of the cutpoint are reported.
            int last = registration.reportedStates.get(uid,
                    ActivityManager.PROCESS_STATE_NONEXISTENT);
            boolean wasBelow = last <= registration.cutpoint;
            boolean below = state <= registration.cutpoint;
            stateReport = last == ActivityManager.PROCESS_STATE_NONEXISTENT || wasBelow != below;
        }
        boolean capabilityReport = (which & ActivityManager.UID_OBSERVER_CAPABILITY) != 0
                && previousCapability != capability;
        if (stateReport || capabilityReport) {
            registration.reportedStates.put(uid, state);
            observer.onUidStateChanged(uid, state, 0L, capability);
        }
    }
}
