package dev.darwinart.runtime.content;

import android.net.Uri;
import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import java.util.ArrayList;

/** System-process owner for the pinned Android 16 IContentService observer contract. */
public final class ContentServiceEndpoint extends Binder {
    private static final String DESCRIPTOR = "android.content.IContentService";
    private static final String OBSERVER_DESCRIPTOR = "android.database.IContentObserver";
    private static final int UNREGISTER = FIRST_CALL_TRANSACTION;
    private static final int REGISTER = FIRST_CALL_TRANSACTION + 1;
    private static final int NOTIFY = FIRST_CALL_TRANSACTION + 2;
    private static final int OBSERVER_ON_CHANGE_ETC = FIRST_CALL_TRANSACTION + 1;
    private final ArrayList<ObserverRecord> observers = new ArrayList<>();

    public ContentServiceEndpoint() { attachInterface(null, DESCRIPTOR); }

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (code == UNREGISTER) {
            data.enforceInterface(DESCRIPTOR);
            IBinder observer = data.readStrongBinder();
            data.enforceNoDataAvail();
            unregister(observer);
            if (reply != null) reply.writeNoException();
            return true;
        }
        if (code == REGISTER) {
            data.enforceInterface(DESCRIPTOR);
            Uri uri = data.readTypedObject(Uri.CREATOR);
            boolean descendants = data.readBoolean();
            IBinder observer = data.readStrongBinder();
            int userId = data.readInt();
            data.readInt(); // targetSdkVersion: permission policy consumes this later.
            data.enforceNoDataAvail();
            register(uri, descendants, observer, userId);
            if (reply != null) reply.writeNoException();
            return true;
        }
        if (code == NOTIFY) {
            data.enforceInterface(DESCRIPTOR);
            Uri[] uris = data.createTypedArray(Uri.CREATOR);
            IBinder source = data.readStrongBinder();
            boolean self = data.readBoolean();
            int changeFlags = data.readInt();
            int userId = data.readInt();
            data.readInt(); // targetSdkVersion
            data.readString(); // callingPackage
            data.enforceNoDataAvail();
            notifyObservers(uris, source, self, changeFlags, userId);
            if (reply != null) reply.writeNoException();
            return true;
        }
        return super.onTransact(code, data, reply, flags);
    }

    private void register(Uri uri, boolean descendants, IBinder observer, int userId)
            throws RemoteException {
        if (uri == null || observer == null) {
            throw new IllegalArgumentException("Content observer requires URI and Binder");
        }
        ObserverRecord record = new ObserverRecord(uri, descendants, observer, userId);
        observer.linkToDeath(record, 0);
        synchronized (observers) { observers.add(record); }
    }

    private void unregister(IBinder observer) {
        if (observer == null) return;
        synchronized (observers) {
            for (int index = observers.size() - 1; index >= 0; --index) {
                ObserverRecord record = observers.get(index);
                if (record.observer == observer) {
                    observers.remove(index);
                    observer.unlinkToDeath(record, 0);
                }
            }
        }
    }

    private void notifyObservers(Uri[] uris, IBinder source, boolean self, int flags, int userId) {
        if (uris == null || uris.length == 0) return;
        ArrayList<ObserverRecord> snapshot;
        synchronized (observers) { snapshot = new ArrayList<>(observers); }
        for (ObserverRecord record : snapshot) {
            if (record.observer == source && !self) continue;
            if (record.userId != userId && record.userId != -1 && userId != -1) continue;
            ArrayList<Uri> matching = new ArrayList<>();
            for (Uri changed : uris) {
                if (changed != null && record.matches(changed)) matching.add(changed);
            }
            if (!matching.isEmpty()) dispatch(record, matching.toArray(new Uri[0]), flags, userId);
        }
    }

    private void dispatch(ObserverRecord record, Uri[] uris, int flags, int userId) {
        Parcel data = Parcel.obtain();
        try {
            data.writeInterfaceToken(OBSERVER_DESCRIPTOR);
            data.writeBoolean(false);
            data.writeTypedArray(uris, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            data.writeInt(flags);
            data.writeInt(userId);
            record.observer.transact(OBSERVER_ON_CHANGE_ETC, data, null, IBinder.FLAG_ONEWAY);
        } catch (RemoteException dead) {
            record.binderDied();
        } finally {
            data.recycle();
        }
    }

    private final class ObserverRecord implements IBinder.DeathRecipient {
        final Uri uri;
        final boolean descendants;
        final IBinder observer;
        final int userId;

        ObserverRecord(Uri uri, boolean descendants, IBinder observer, int userId) {
            this.uri = uri;
            this.descendants = descendants;
            this.observer = observer;
            this.userId = userId;
        }

        boolean matches(Uri changed) {
            if (uri.equals(changed)) return true;
            if (!descendants || !uri.getScheme().equals(changed.getScheme())
                    || !uri.getAuthority().equals(changed.getAuthority())) return false;
            String parent = uri.getPath();
            String child = changed.getPath();
            if (parent == null || parent.isEmpty()) return true;
            return child != null && child.startsWith(parent)
                    && (child.length() == parent.length()
                    || parent.endsWith("/") || child.charAt(parent.length()) == '/');
        }

        @Override public void binderDied() { unregister(observer); }
    }
}
