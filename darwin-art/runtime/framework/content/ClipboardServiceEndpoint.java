package dev.darwinart.runtime.content;

import android.content.ClipData;
import android.content.ClipDescription;
import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import java.util.ArrayList;
import java.util.HashMap;

/** System-process owner for the pinned Android 16 IClipboard contract. */
public final class ClipboardServiceEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.content.IClipboard";
    private static final String LISTENER_DESCRIPTOR =
            "android.content.IOnPrimaryClipChangedListener";
    private static final int SET_PRIMARY_CLIP = FIRST_CALL_TRANSACTION;
    private static final int SET_PRIMARY_CLIP_AS_PACKAGE = FIRST_CALL_TRANSACTION + 1;
    private static final int CLEAR_PRIMARY_CLIP = FIRST_CALL_TRANSACTION + 2;
    private static final int GET_PRIMARY_CLIP = FIRST_CALL_TRANSACTION + 3;
    private static final int GET_PRIMARY_CLIP_DESCRIPTION = FIRST_CALL_TRANSACTION + 4;
    private static final int HAS_PRIMARY_CLIP = FIRST_CALL_TRANSACTION + 5;
    private static final int ADD_LISTENER = FIRST_CALL_TRANSACTION + 6;
    private static final int REMOVE_LISTENER = FIRST_CALL_TRANSACTION + 7;
    private static final int HAS_CLIPBOARD_TEXT = FIRST_CALL_TRANSACTION + 8;
    private static final int GET_PRIMARY_CLIP_SOURCE = FIRST_CALL_TRANSACTION + 9;
    private static final int GET_ACCESS_NOTIFICATIONS_ENABLED = FIRST_CALL_TRANSACTION + 10;
    private static final int SET_ACCESS_NOTIFICATIONS_ENABLED = FIRST_CALL_TRANSACTION + 11;
    private static final int DISPATCH_PRIMARY_CLIP_CHANGED = FIRST_CALL_TRANSACTION;

    private final HashMap<Long, ClipRecord> clips = new HashMap<>();
    private final ArrayList<ListenerRecord> listeners = new ArrayList<>();
    private final HashMap<Integer, Boolean> accessNotifications = new HashMap<>();

    public ClipboardServiceEndpoint() { attachInterface(null, DESCRIPTOR); }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == SET_PRIMARY_CLIP || code == SET_PRIMARY_CLIP_AS_PACKAGE) {
            data.enforceInterface(DESCRIPTOR);
            ClipData clip = data.readTypedObject(ClipData.CREATOR);
            String callingPackage = data.readString();
            data.readString(); // attributionTag
            int userId = data.readInt();
            int deviceId = data.readInt();
            String sourcePackage = code == SET_PRIMARY_CLIP_AS_PACKAGE
                    ? data.readString() : callingPackage;
            data.enforceNoDataAvail();
            setPrimaryClip(clip, sourcePackage, userId, deviceId);
            writeSuccess(reply);
            return true;
        }
        if (code == CLEAR_PRIMARY_CLIP) {
            data.enforceInterface(DESCRIPTOR);
            data.readString(); // callingPackage
            data.readString(); // attributionTag
            int userId = data.readInt();
            int deviceId = data.readInt();
            data.enforceNoDataAvail();
            setPrimaryClip(null, null, userId, deviceId);
            writeSuccess(reply);
            return true;
        }
        if (code >= GET_PRIMARY_CLIP && code <= HAS_PRIMARY_CLIP) {
            data.enforceInterface(DESCRIPTOR);
            data.readString(); // callingPackage
            data.readString(); // attributionTag
            int userId = data.readInt();
            int deviceId = data.readInt();
            data.enforceNoDataAvail();
            ClipRecord record;
            synchronized (clips) { record = clips.get(key(userId, deviceId)); }
            reply.writeNoException();
            if (code == GET_PRIMARY_CLIP) {
                reply.writeTypedObject(record == null ? null : record.clip,
                        Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            } else if (code == GET_PRIMARY_CLIP_DESCRIPTION) {
                reply.writeTypedObject(record == null ? null : record.clip.getDescription(),
                        Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            } else {
                reply.writeBoolean(record != null && record.clip != null);
            }
            return true;
        }
        if (code == ADD_LISTENER || code == REMOVE_LISTENER) {
            data.enforceInterface(DESCRIPTOR);
            IBinder listener = data.readStrongBinder();
            String callingPackage = data.readString();
            data.readString(); // attributionTag
            int userId = data.readInt();
            int deviceId = data.readInt();
            data.enforceNoDataAvail();
            if (code == ADD_LISTENER) {
                addListener(listener, callingPackage, userId, deviceId);
            } else {
                removeListener(listener);
            }
            writeSuccess(reply);
            return true;
        }
        if (code == HAS_CLIPBOARD_TEXT || code == GET_PRIMARY_CLIP_SOURCE) {
            data.enforceInterface(DESCRIPTOR);
            data.readString(); // callingPackage
            data.readString(); // attributionTag
            int userId = data.readInt();
            int deviceId = data.readInt();
            data.enforceNoDataAvail();
            ClipRecord record;
            synchronized (clips) { record = clips.get(key(userId, deviceId)); }
            reply.writeNoException();
            if (code == HAS_CLIPBOARD_TEXT) {
                boolean hasText = record != null && record.clip != null
                        && record.clip.getItemCount() > 0
                        && record.clip.getItemAt(0).getText() != null;
                reply.writeBoolean(hasText);
            } else {
                reply.writeString(record == null ? null : record.sourcePackage);
            }
            return true;
        }
        if (code == GET_ACCESS_NOTIFICATIONS_ENABLED) {
            data.enforceInterface(DESCRIPTOR);
            int userId = data.readInt();
            data.enforceNoDataAvail();
            synchronized (accessNotifications) {
                reply.writeNoException();
                reply.writeBoolean(accessNotifications.getOrDefault(userId, Boolean.TRUE));
            }
            return true;
        }
        if (code == SET_ACCESS_NOTIFICATIONS_ENABLED) {
            data.enforceInterface(DESCRIPTOR);
            boolean enabled = data.readBoolean();
            int userId = data.readInt();
            data.enforceNoDataAvail();
            synchronized (accessNotifications) { accessNotifications.put(userId, enabled); }
            writeSuccess(reply);
            return true;
        }
        return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
    }

    private static long key(int userId, int deviceId) {
        return ((long) userId << 32) | (deviceId & 0xffffffffL);
    }

    private static void writeSuccess(Parcel reply) {
        if (reply != null) reply.writeNoException();
    }

    private void setPrimaryClip(ClipData clip, String sourcePackage, int userId, int deviceId) {
        synchronized (clips) {
            long key = key(userId, deviceId);
            if (clip == null) clips.remove(key);
            else clips.put(key, new ClipRecord(clip, sourcePackage));
        }
        notifyListeners(userId, deviceId);
    }

    private void addListener(IBinder listener, String packageName, int userId, int deviceId)
            throws RemoteException {
        if (listener == null) throw new IllegalArgumentException("clipboard listener is null");
        synchronized (listeners) {
            for (ListenerRecord record : listeners) {
                if (record.listener == listener && record.userId == userId
                        && record.deviceId == deviceId) return;
            }
            ListenerRecord record = new ListenerRecord(listener, packageName, userId, deviceId);
            listener.linkToDeath(record, 0);
            listeners.add(record);
        }
    }

    private void removeListener(IBinder listener) {
        if (listener == null) return;
        synchronized (listeners) {
            for (int index = listeners.size() - 1; index >= 0; --index) {
                ListenerRecord record = listeners.get(index);
                if (record.listener == listener) {
                    listeners.remove(index);
                    listener.unlinkToDeath(record, 0);
                }
            }
        }
    }

    private void notifyListeners(int userId, int deviceId) {
        ArrayList<ListenerRecord> snapshot;
        synchronized (listeners) { snapshot = new ArrayList<>(listeners); }
        for (ListenerRecord record : snapshot) {
            if (record.userId != userId || record.deviceId != deviceId) continue;
            Parcel data = Parcel.obtain();
            try {
                data.writeInterfaceToken(LISTENER_DESCRIPTOR);
                record.listener.transact(
                        DISPATCH_PRIMARY_CLIP_CHANGED, data, null, IBinder.FLAG_ONEWAY);
            } catch (RemoteException dead) {
                record.binderDied();
            } finally {
                data.recycle();
            }
        }
    }

    private static final class ClipRecord {
        final ClipData clip;
        final String sourcePackage;

        ClipRecord(ClipData clip, String sourcePackage) {
            this.clip = clip;
            this.sourcePackage = sourcePackage;
        }
    }

    private final class ListenerRecord implements IBinder.DeathRecipient {
        final IBinder listener;
        final String packageName;
        final int userId;
        final int deviceId;

        ListenerRecord(IBinder listener, String packageName, int userId, int deviceId) {
            this.listener = listener;
            this.packageName = packageName;
            this.userId = userId;
            this.deviceId = deviceId;
        }

        @Override public void binderDied() { removeListener(listener); }
    }
}
