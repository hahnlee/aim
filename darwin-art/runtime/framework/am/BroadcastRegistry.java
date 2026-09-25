package dev.darwinart.runtime.am;

import android.content.Intent;
import android.content.IntentFilter;
import android.os.IBinder;
import android.os.RemoteException;
import android.util.Log;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;

/**
 * Runtime-registered broadcast receivers and sticky broadcasts
 * (ActivityManagerService.registerReceiverWithFeature / BroadcastQueue).
 *
 * <p>Delivery covers unordered broadcasts to registered receivers. Ordered
 * broadcasts (result receivers) and manifest-declared receivers need the
 * receiver-process launch and finishReceiver chain, which this owner does not
 * implement; callers report those requests as unsupported.</p>
 */
final class BroadcastRegistry {
    private static final String TAG = "BroadcastQueue";
    // Context.RECEIVER_* registration flags.
    static final int RECEIVER_EXPORTED = 0x2;
    static final int RECEIVER_NOT_EXPORTED = 0x4;
    // Process.SYSTEM_UID: system broadcasts reach non-exported receivers.
    private static final int SYSTEM_UID = 1000;

    /** Transport for IApplicationThread.scheduleRegisteredReceiver. */
    interface Delivery {
        void scheduleRegisteredReceiver(IBinder applicationThread, IBinder receiver,
                Intent intent, boolean sticky, int sendingUser, int sendingUid,
                String sendingPackage) throws RemoteException;
    }

    private static final class Registration {
        final int pid;
        final int uid;
        final String packageName;
        final IBinder thread;
        final IBinder receiver;
        final IntentFilter filter;
        final String requiredPermission;
        final int userId;
        final boolean exported;

        Registration(ApplicationProcessRegistry.AttachedApplication caller, IBinder receiver,
                IntentFilter filter, String requiredPermission, int userId, boolean exported) {
            pid = caller.pid;
            uid = caller.uid;
            packageName = caller.packageName;
            thread = caller.thread;
            this.receiver = receiver;
            this.filter = filter;
            this.requiredPermission = requiredPermission;
            this.userId = userId;
            this.exported = exported;
        }
    }

    /** A sticky broadcast and the sender identity its redelivery reports. */
    private static final class Sticky {
        final Intent intent;
        final int sendingUid;
        final String sendingPackage;
        final int userId;

        Sticky(Intent intent, int sendingUid, String sendingPackage, int userId) {
            this.intent = intent;
            this.sendingUid = sendingUid;
            this.sendingPackage = sendingPackage;
            this.userId = userId;
        }
    }

    // UserHandle.USER_ALL.
    static final int USER_ALL = -1;

    private final Delivery delivery;
    private final List<Registration> registrations = new ArrayList<>();
    private final List<Sticky> stickies = new ArrayList<>();

    BroadcastRegistry(Delivery delivery) {
        this.delivery = delivery;
    }

    /**
     * Registers {@code receiver} (when non-null) for {@code filter} and returns
     * the first current sticky broadcast the filter matches. As in AMS, every
     * matching sticky is also delivered to the new receiver.
     */
    Intent register(ApplicationProcessRegistry.AttachedApplication caller, IBinder receiver,
            IntentFilter filter, String requiredPermission, int userId, int flags) {
        if (filter == null) throw new NullPointerException("filter");
        boolean exported = (flags & RECEIVER_EXPORTED) != 0;
        if (exported && (flags & RECEIVER_NOT_EXPORTED) != 0) {
            throw new IllegalArgumentException(
                    "Receiver can't specify both RECEIVER_EXPORTED and RECEIVER_NOT_EXPORTED"
                            + "flag");
        }
        // Without either flag, Android before 14 treats the receiver as
        // exported. The Android 14 requirement to choose explicitly depends on
        // the protected-broadcast list, which is not modeled here.
        if ((flags & RECEIVER_NOT_EXPORTED) == 0) exported = true;
        List<Sticky> matching = new ArrayList<>();
        Registration registration;
        synchronized (this) {
            for (Sticky candidate : stickies) {
                if (userMatches(candidate.userId, userId) && matches(filter, candidate.intent)) {
                    matching.add(candidate);
                }
            }
            Intent sticky = matching.isEmpty() ? null : new Intent(matching.get(0).intent);
            if (receiver == null) return sticky;
            for (Registration existing : registrations) {
                if (existing.receiver.equals(receiver) && existing.pid != caller.pid) {
                    throw new IllegalArgumentException(
                            "Receiver requested to register for process " + caller.pid
                                    + " was previously registered for process " + existing.pid);
                }
            }
            registration = new Registration(caller, receiver, filter, requiredPermission,
                    userId, exported);
            registrations.add(registration);
        }
        try {
            receiver.linkToDeath(() -> unregister(receiver), 0);
        } catch (RemoteException gone) {
            unregister(receiver);
            return matching.isEmpty() ? null : new Intent(matching.get(0).intent);
        }
        for (Sticky sticky : matching) {
            deliver(registration, sticky.intent, true, sticky.userId, sticky.sendingUid,
                    sticky.sendingPackage);
        }
        return matching.isEmpty() ? null : new Intent(matching.get(0).intent);
    }

    synchronized void unregister(IBinder receiver) {
        if (receiver == null) return;
        registrations.removeIf(registration -> registration.receiver.equals(receiver));
    }

    synchronized void processGone(int pid) {
        registrations.removeIf(registration -> registration.pid == pid);
    }

    /**
     * Unordered delivery to every matching registered receiver the sender may
     * reach; a sticky broadcast also replaces the stored sticky intent that
     * {@link Intent#filterEquals} it.
     */
    void broadcast(int sendingUid, String sendingPackage, Intent intent, String resolvedType,
            boolean permissionGated, boolean sticky, int userId) {
        if (intent == null) throw new IllegalArgumentException("intent is null");
        if (intent.hasFileDescriptors()) {
            throw new IllegalArgumentException("File descriptors passed in Intent");
        }
        List<Registration> targets = new ArrayList<>();
        synchronized (this) {
            if (sticky) {
                for (Iterator<Sticky> it = stickies.iterator(); it.hasNext();) {
                    Sticky existing = it.next();
                    if (existing.userId == userId && existing.intent.filterEquals(intent)) {
                        it.remove();
                    }
                }
                stickies.add(new Sticky(new Intent(intent), sendingUid, sendingPackage, userId));
            }
            String targetPackage = intent.getPackage();
            for (Registration registration : registrations) {
                if (!userMatches(registration.userId, userId)) continue;
                if (!registration.exported && registration.uid != sendingUid
                        && sendingUid != SYSTEM_UID) {
                    continue;
                }
                // Permission grants are not modeled; a permission-guarded
                // receiver, or a broadcast requiring receiver permissions,
                // reaches only its own uid.
                if ((registration.requiredPermission != null || permissionGated)
                        && registration.uid != sendingUid) {
                    continue;
                }
                if (targetPackage != null && !targetPackage.equals(registration.packageName)) {
                    continue;
                }
                if (!matches(registration.filter, intent, resolvedType)) continue;
                targets.add(registration);
            }
        }
        for (Registration target : targets) {
            deliver(target, intent, sticky, userId, sendingUid, sendingPackage);
        }
    }

    private void deliver(Registration target, Intent intent, boolean sticky, int userId,
            int sendingUid, String sendingPackage) {
        try {
            delivery.scheduleRegisteredReceiver(target.thread, target.receiver,
                    new Intent(intent), sticky, userId, sendingUid, sendingPackage);
        } catch (RemoteException gone) {
            Log.w(TAG, "Failure sending broadcast " + intent + " to pid " + target.pid, gone);
            unregister(target.receiver);
        }
    }

    private static boolean userMatches(int first, int second) {
        return first == USER_ALL || second == USER_ALL || first == second;
    }

    private static boolean matches(IntentFilter filter, Intent intent) {
        return matches(filter, intent, intent.getType());
    }

    private static boolean matches(IntentFilter filter, Intent intent, String resolvedType) {
        return filter.match(intent.getAction(), resolvedType, intent.getScheme(),
                intent.getData(), intent.getCategories(), TAG) >= 0;
    }
}
