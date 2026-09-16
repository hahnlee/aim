package dev.darwinart.runtime.user;

import android.os.Binder;
import android.os.Bundle;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import java.lang.reflect.Field;

/** UserManager Binder state for the single unlocked Android user. */
public final class UserManagerEndpoint extends Binder {
    private final int isUserUnlockedCode = transaction("isUserUnlocked");
    private final int isUserUnlockingOrUnlockedCode =
            transaction("isUserUnlockingOrUnlocked");
    private final int getProfileIdsCode = transaction("getProfileIds");
    private final int getProfileIdsExcludingHiddenCode =
            transaction("getProfileIdsExcludingHidden");
    private final int getApplicationRestrictionsCode =
            transaction("getApplicationRestrictions");
    private final int getApplicationRestrictionsForUserCode =
            transaction("getApplicationRestrictionsForUser");

    public UserManagerEndpoint() {
        attachInterface(null, "android.os.IUserManager");
    }

    private static int transaction(String name) {
        try {
            Field field = Class.forName("android.os.IUserManager$Stub")
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
        if (code != isUserUnlockedCode && code != isUserUnlockingOrUnlockedCode
                && code != getProfileIdsCode && code != getProfileIdsExcludingHiddenCode) {
            if (code == getApplicationRestrictionsCode
                    || code == getApplicationRestrictionsForUserCode) {
                data.enforceInterface("android.os.IUserManager");
                data.readString(); // Package name; no policy is installed for user zero.
                if (code == getApplicationRestrictionsForUserCode) data.readInt();
                data.enforceNoDataAvail();
                reply.writeNoException();
                reply.writeTypedObject(new Bundle(), Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
                return true;
            }
            return super.onTransact(code, data, reply, flags);
        }
        data.enforceInterface("android.os.IUserManager");
        int userId = data.readInt();
        if (code == getProfileIdsCode || code == getProfileIdsExcludingHiddenCode) {
            data.readBoolean(); // enabledOnly; user zero is always enabled.
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeIntArray(userId == 0 ? new int[] {0} : new int[0]);
            return true;
        }
        data.enforceNoDataAvail();
        reply.writeNoException();
        reply.writeBoolean(userId == 0);
        return true;
    }
}
