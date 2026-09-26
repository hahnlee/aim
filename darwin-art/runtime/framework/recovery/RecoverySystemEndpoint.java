package dev.darwinart.runtime.recovery;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.RemoteException;
import android.util.Log;
import java.lang.reflect.Field;

/**
 * The "recovery" service (RecoverySystemService). A profile has no recovery
 * partition or bootloader control block, and wiping it would delete the
 * user's Android data, so every request that would reboot into recovery,
 * wipe user data or apply an update is refused and reported, never
 * acknowledged. RecoverySystem.rebootWipeUserData therefore fails with its
 * own "Setup BCB failed" IOException instead of a NullPointerException for a
 * missing service (for example UserDataPreparer after a failed prepare of
 * user 0).
 */
public final class RecoverySystemEndpoint extends Binder {
    static final String DESCRIPTOR = "android.os.IRecoverySystem";
    private static final String TAG = "DarwinRecoverySystem";
    // RecoverySystem.RESUME_ON_REBOOT_REBOOT_ERROR_UNSPECIFIED.
    private static final int REBOOT_ERROR_UNSPECIFIED = 1000;

    private final int uncryptCode = transaction("uncrypt");
    private final int setupBcbCode = transaction("setupBcb");
    private final int clearBcbCode = transaction("clearBcb");
    private final int rebootRecoveryWithCommandCode = transaction("rebootRecoveryWithCommand");
    private final int requestLskfCode = transaction("requestLskf");
    private final int clearLskfCode = transaction("clearLskf");
    private final int isLskfCapturedCode = transaction("isLskfCaptured");
    private final int rebootWithLskfAssumeSlotSwitchCode =
            transaction("rebootWithLskfAssumeSlotSwitch");
    private final int rebootWithLskfCode = transaction("rebootWithLskf");
    private final int allocateSpaceForUpdateCode = transaction("allocateSpaceForUpdate");

    public RecoverySystemEndpoint() {
        attachInterface(null, DESCRIPTOR);
    }

    private static int transaction(String name) {
        try {
            Field field = Class.forName("android.os.IRecoverySystem$Stub")
                    .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code < FIRST_CALL_TRANSACTION || code > LAST_CALL_TRANSACTION) {
            return super.onTransact(code, data, reply, flags);
        }
        data.enforceInterface(DESCRIPTOR);
        if (code == setupBcbCode || code == rebootRecoveryWithCommandCode) {
            String command = data.readString();
            data.enforceNoDataAvail();
            Log.e(TAG, "refused recovery command from pid " + Binder.getCallingPid()
                    + ": " + command);
            if (code == rebootRecoveryWithCommandCode) {
                throw new SecurityException("recovery is unavailable; command refused");
            }
            reply.writeNoException();
            reply.writeBoolean(false);
            return true;
        }
        if (code == clearBcbCode) {
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeBoolean(true); // There is no control block to clear.
            return true;
        }
        if (code == uncryptCode) {
            data.readString(); // Package path.
            data.readStrongBinder(); // Progress listener.
            data.enforceNoDataAvail();
            return refuse(reply, "uncrypt");
        }
        if (code == requestLskfCode) {
            data.readString(); // Caller id.
            data.readTypedObject(android.content.IntentSender.CREATOR);
            data.enforceNoDataAvail();
            return refuse(reply, "requestLskf");
        }
        if (code == clearLskfCode || code == isLskfCapturedCode
                || code == allocateSpaceForUpdateCode) {
            data.readString();
            data.enforceNoDataAvail();
            return refuse(reply, "update preparation");
        }
        if (code == rebootWithLskfAssumeSlotSwitchCode || code == rebootWithLskfCode) {
            data.readString(); // Package name.
            data.readString(); // Reason.
            if (code == rebootWithLskfCode) data.readBoolean(); // Slot switch.
            data.enforceNoDataAvail();
            Log.e(TAG, "refused reboot to apply an update from pid " + Binder.getCallingPid());
            reply.writeNoException();
            reply.writeInt(REBOOT_ERROR_UNSPECIFIED);
            return true;
        }
        return super.onTransact(code, data, reply, flags);
    }

    private static boolean refuse(Parcel reply, String request) {
        Log.w(TAG, "refused " + request + " from pid " + Binder.getCallingPid());
        reply.writeNoException();
        reply.writeBoolean(false);
        return true;
    }
}
