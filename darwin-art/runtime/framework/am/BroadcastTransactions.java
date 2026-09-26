package dev.darwinart.runtime.am;

import android.app.IApplicationThread;
import android.content.IIntentReceiver;
import android.content.Intent;
import android.content.IntentFilter;
import android.os.Binder;
import android.os.Bundle;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import dev.darwinart.runtime.wm.ActivityClientControllerEndpoint;

/** IActivityManager receiver registration and broadcast transactions. */
final class BroadcastTransactions implements SystemBroadcasts {
    // ActivityManager.BROADCAST_SUCCESS.
    private static final int BROADCAST_SUCCESS = 0;
    // Process.SYSTEM_UID.
    private static final int SYSTEM_UID = 1000;

    private final ApplicationProcessRegistry processes;
    final BroadcastRegistry registry = new BroadcastRegistry(
            BroadcastTransactions::scheduleRegisteredReceiver, ApplicationPackages::hasPermission);

    BroadcastTransactions(ApplicationProcessRegistry processes) {
        this.processes = processes;
    }

    /** registerReceiverWithFeature: returns the first matching sticky intent. */
    void register(Parcel data, Parcel reply) {
        IBinder callerThread = data.readStrongBinder();
        String callerPackage = data.readString();
        data.readString(); // attribution feature id
        data.readString(); // receiver id
        IBinder receiver = data.readStrongBinder();
        IntentFilter filter = data.readTypedObject(IntentFilter.CREATOR);
        String requiredPermission = data.readString();
        int userId = data.readInt();
        int flags = data.readInt();
        data.enforceNoDataAvail();
        ApplicationProcessRegistry.AttachedApplication caller =
                processes.requireCaller(Binder.getCallingPid(), callerThread, callerPackage);
        Intent sticky = registry.register(caller, receiver, filter, requiredPermission, userId,
                flags);
        reply.writeNoException();
        reply.writeTypedObject(sticky, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
    }

    void unregister(Parcel data, Parcel reply) {
        IBinder receiver = data.readStrongBinder();
        data.enforceNoDataAvail();
        registry.unregister(receiver);
        reply.writeNoException();
    }

    /**
     * broadcastIntentWithFeature. Returns false for ordered broadcasts (a
     * result receiver or serialized delivery), which this owner does not run.
     */
    boolean broadcast(Parcel data, Parcel reply) {
        data.readStrongBinder(); // caller thread; identity comes from the calling pid
        data.readString(); // attribution feature id
        Intent intent = data.readTypedObject(Intent.CREATOR);
        String resolvedType = data.readString();
        IBinder resultTo = data.readStrongBinder();
        data.readInt(); // initial result code
        data.readString(); // initial result data
        data.readTypedObject(Bundle.CREATOR); // initial result extras
        String[] requiredPermissions = data.createStringArray();
        data.createStringArray(); // excluded permissions
        data.createStringArray(); // excluded packages
        data.readInt(); // app op
        data.readTypedObject(Bundle.CREATOR); // BroadcastOptions
        boolean serialized = data.readBoolean();
        boolean sticky = data.readBoolean();
        int userId = data.readInt();
        data.enforceNoDataAvail();
        if (resultTo != null || serialized) return false;
        // As in AMS, a process may broadcast once attachApplication has
        // identified it, including from Application.onCreate.
        ApplicationProcessRegistry.AttachedApplication caller =
                processes.requireIdentifiedAttachment(Binder.getCallingPid(),
                        Binder.getCallingUid());
        registry.broadcast(caller.uid, caller.packageName, intent, resolvedType,
                requiredPermissions, null, null, sticky, userId);
        reply.writeNoException();
        reply.writeInt(BROADCAST_SUCCESS);
        return true;
    }

    @Override
    public void broadcastAsSystem(Intent intent, boolean sticky) {
        registry.broadcast(SYSTEM_UID, "android", intent, null, null, null, null, sticky,
                BroadcastRegistry.USER_ALL);
    }

    /**
     * ActivityManagerInternal.broadcastIntentWithCallback: an unordered
     * broadcast from the system; {@code resultTo} receives the intent once
     * every receiver has been sent it, as the final receiver of a
     * non-ordered broadcast does.
     */
    int broadcastWithCallback(Intent intent, IIntentReceiver resultTo,
            String[] requiredPermissions, int userId, int[] appIdAllowList,
            java.util.function.BiFunction<Integer, Bundle, Bundle> filterExtrasForReceiver) {
        BroadcastRegistry.ReceiverIntent receiverIntent = filterExtrasForReceiver == null
                ? null
                : (uid, sent) -> {
                    if (sent.getExtras() == null) return sent;
                    Bundle extras = filterExtrasForReceiver.apply(uid, sent.getExtras());
                    return extras == null ? null : new Intent(sent).replaceExtras(extras);
                };
        registry.broadcast(SYSTEM_UID, "android", intent, null, requiredPermissions,
                appIdAllowList, receiverIntent, false, userId);
        if (resultTo != null) {
            try {
                // Activity.RESULT_OK, the initial result of a system broadcast.
                resultTo.performReceive(intent, -1, null, null, false, false, userId);
            } catch (RemoteException ignored) {
                // The final receiver's process is gone.
            }
        }
        return BROADCAST_SUCCESS;
    }

    private static void scheduleRegisteredReceiver(IBinder thread, IBinder receiver,
            Intent intent, boolean sticky, int sendingUser, int sendingUid,
            String sendingPackage) throws RemoteException {
        // Unordered delivery: the receiver is assumed delivered and does not
        // report finishReceiver.
        IApplicationThread.Stub.asInterface(thread).scheduleRegisteredReceiver(
                IIntentReceiver.Stub.asInterface(receiver), intent, 0, null, null,
                false, sticky, true, sendingUser,
                RunningAppProcesses.processState(
                        ActivityClientControllerEndpoint.activityPresence(thread)),
                sendingUid, sendingPackage);
    }
}
