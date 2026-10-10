package dev.aim.server;

import android.content.Context;
import android.database.ContentObserver;
import android.os.Binder;
import android.os.Handler;
import android.os.Parcel;
import android.os.RemoteException;
import android.provider.Settings;
import com.android.server.LocalServices;
import com.android.server.pm.UserManagerInternal;

/** Original Settings/UM leaf; never reads PMS or its snapshot. */
final class WebInstantAppsState extends IWebInstantAppsState.Stub {
    private final Context context;
    private final ContentObserver observer;
    private IWebInstantAppsChanged callback;
    private long epoch;
    private boolean ready;
    private boolean registered;
    private IllegalStateException failure;

    WebInstantAppsState(Context context, Handler handler) {
        this.context = context;
        observer = new ContentObserver(handler) {
            @Override public void onChange(boolean selfChange) { publishChanged(); }
        };
    }
    private static void enforceSystem() {
        if (Binder.getCallingUid() != 1000) throw new SecurityException("Web instant state requires system uid");
    }
    private synchronized byte[] read() {
        if (failure != null) throw failure;
        if (!ready) {
            // Original mWebInstantAppsDisabled is constructed empty; epoch zero
            // denotes this constructor state, never a Settings snapshot.
            Parcel out = Parcel.obtain();
            try {
                out.writeInt(1);
                out.writeLong(0);
                out.writeInt(0);
                return out.marshall();
            } finally { out.recycle(); }
        }
        UserManagerInternal users = LocalServices.getService(UserManagerInternal.class);
        if (users == null) throw new IllegalStateException("Web instant policy UM owner absent");
        int[] ids = users.getUserIds();
        if (ids == null || ids.length == 0) throw new IllegalStateException("Web instant policy UM inventory absent");
        boolean globalDisabled = Settings.Global.getInt(context.getContentResolver(),
                "enable_ephemeral_feature", 1) == 0;
        if (epoch == Long.MAX_VALUE) throw new IllegalStateException("Web instant policy epoch exhausted");
        Parcel out = Parcel.obtain();
        try {
            out.writeInt(1);
            out.writeLong(++epoch);
            out.writeInt(ids.length);
            for (int user : ids) {
                if (user < 0) throw new IllegalStateException("Invalid original UM user id");
                out.writeInt(user);
                out.writeBoolean(globalDisabled || Settings.Secure.getIntForUser(
                        context.getContentResolver(), "instant_apps_enabled", 1, user) == 0);
            }
            if (users != LocalServices.getService(UserManagerInternal.class))
                throw new IllegalStateException("Web instant policy UM owner replaced");
            return out.marshall();
        } finally { out.recycle(); }
    }
    @Override public byte[] capture() { enforceSystem(); return read(); }
    @Override public synchronized void start(IWebInstantAppsChanged target) {
        enforceSystem();
        if (target == null) throw new NullPointerException("Web instant policy callback");
        if (callback != null) throw new IllegalStateException("Web instant policy observer already installed");
        context.getContentResolver().registerContentObserver(Settings.Global.getUriFor(
                "enable_ephemeral_feature"), false, observer, -1);
        try {
            context.getContentResolver().registerContentObserver(Settings.Secure.getUriFor(
                    "instant_apps_enabled"), false, observer, -1);
        } catch (RuntimeException error) {
            context.getContentResolver().unregisterContentObserver(observer);
            throw error;
        }
        registered = true;
        callback = target;
        ready = true;
        publishChanged();
        if (failure != null) throw failure;
    }
    private void publishChanged() {
        final IWebInstantAppsChanged target;
        final byte[] record;
        synchronized (this) {
            target = callback;
            if (target == null) return;
            record = read();
        }
        try { target.changed(record); }
        catch (RemoteException | RuntimeException error) {
            synchronized (this) { failure = new IllegalStateException("Web instant state publication failed", error); }
        }
    }
    @Override public synchronized void stop() {
        enforceSystem();
        if (registered) context.getContentResolver().unregisterContentObserver(observer);
        registered = false;
        callback = null;
    }
}
