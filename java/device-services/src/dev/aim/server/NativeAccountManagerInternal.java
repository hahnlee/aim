package dev.aim.server;

import android.accounts.Account;
import android.accounts.AccountManagerInternal;
import android.os.RemoteCallback;
import android.os.RemoteException;
import java.util.Objects;

/** Inactive local adapter over the same native owner as NativeSyncAccounts (#1460). */
final class NativeAccountManagerInternal extends AccountManagerInternal {
    private final ISyncAccounts owner;
    NativeAccountManagerInternal(ISyncAccounts owner) { this.owner = Objects.requireNonNull(owner); }
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
        throw new UnsupportedOperationException("account consent (#1504, #1460 slice 3d)");
    }
    @Override public byte[] backupAccountAccessPermissions(int userId) {
        throw new UnsupportedOperationException("account permission backup (#1504)");
    }
    @Override public void restoreAccountAccessPermissions(byte[] data, int userId) {
        throw new UnsupportedOperationException("account permission restore (#1504)");
    }
}
