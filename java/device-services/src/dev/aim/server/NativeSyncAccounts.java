package dev.aim.server;

import android.accounts.Account;
import android.accounts.AccountAndUser;
import android.os.RemoteException;
import com.android.server.content.SyncAccounts;
import java.util.Objects;

/** Typed adapter for the image sync seam (#1460). */
final class NativeSyncAccounts implements SyncAccounts {
    private final ISyncAccounts owner;
    NativeSyncAccounts(ISyncAccounts owner) { this.owner = Objects.requireNonNull(owner); }
    @Override public void validateAccounts(int userId) {
        try { owner.validateAccounts(userId); }
        catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
    @Override public Account[] getAccounts(int userId, String opPackageName) {
        try { return Objects.requireNonNull(owner.getAccounts(userId, opPackageName)); }
        catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
    private static AccountAndUser[] systemAccounts(Account[] accounts) {
        // The native user owner has exactly one running/alive user, including while locked.
        AccountAndUser[] result = new AccountAndUser[accounts.length];
        for (int i = 0; i < accounts.length; i++) result[i] = new AccountAndUser(accounts[i], 0);
        return result;
    }
    @Override public AccountAndUser[] getRunningAccountsForSystem() {
        try { return systemAccounts(owner.getRunningAccountsForSystem()); }
        catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
    @Override public AccountAndUser[] getAllAccountsForSystemProcess() {
        try { return systemAccounts(owner.getAllAccountsForSystemProcess()); }
        catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
}
