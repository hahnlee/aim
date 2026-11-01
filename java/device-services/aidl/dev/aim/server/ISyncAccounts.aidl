package dev.aim.server;
import android.accounts.Account;
import android.os.IRemoteCallback;
import dev.aim.server.IAccountConsentNotification;
import dev.aim.server.IAccountPermissionListener;
/** Strict snapshots and access decisions from the native account owner; system UID only. */
interface ISyncAccounts {
    void validateAccounts(int userId);
    Account[] getAccounts(int userId, String opPackageName);
    Account[] getRunningAccountsForSystem();
    Account[] getAllAccountsForSystemProcess();
    boolean hasAccountAccess(in Account account, int uid);
    void addOnAppPermissionChangeListener(IAccountPermissionListener listener);
    void requestAccountAccess(in Account account, String packageName, int userId, IRemoteCallback callback, IAccountConsentNotification notification);
}
