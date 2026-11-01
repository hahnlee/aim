package dev.aim.server;

import android.accounts.Account;
import android.accounts.AccountManagerInternal;
import android.os.RemoteCallback;
import android.os.IRemoteCallback;
import android.os.Bundle;
import android.content.Context;
import android.os.RemoteException;
import java.util.Objects;

/** Local adapter over the same native owner as NativeSyncAccounts (#1460). */
final class NativeAccountManagerInternal extends AccountManagerInternal {
    private final ISyncAccounts owner;
    private final IAccountConsentNotification notifications;
    NativeAccountManagerInternal(Context context, ISyncAccounts owner) {
        this.notifications = new AccountConsentNotifications(context);
        this.owner = Objects.requireNonNull(owner);
    }
    @Override public boolean hasAccountAccess(Account account, int uid) {
        try { return owner.hasAccountAccess(Objects.requireNonNull(account), uid); }
        catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
    @Override public void addOnAppPermissionChangeListener(OnAppPermissionChangeListener listener) {
        Objects.requireNonNull(listener);
        try {
            owner.addOnAppPermissionChangeListener(new IAccountPermissionListener.Stub() {
                @Override public void onAppPermissionChanged(Account account, int uid) {
                    listener.onAppPermissionChanged(account, uid);
                }
            });
        } catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
    @Override public void requestAccountAccess(Account account, String packageName, int userId, RemoteCallback callback) {
        if (account == null || packageName == null || userId < 0 || callback == null) return;
        try {
            owner.requestAccountAccess(account, packageName, userId, new IRemoteCallback.Stub() {
                @Override public void sendResult(Bundle result) { callback.sendResult(result); }
            }, notifications);
        } catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
    @Override public byte[] backupAccountAccessPermissions(int userId) {
        throw new UnsupportedOperationException("account permission backup (#1504)");
    }
    @Override public void restoreAccountAccessPermissions(byte[] data, int userId) {
        throw new UnsupportedOperationException("account permission restore (#1504)");
    }
}
