package dev.aim.server;
import android.accounts.Account;
/** Strict snapshots from the native account owner; system UID only. */
interface ISyncAccounts {
    void validateAccounts(int userId);
    Account[] getAccounts(int userId, String opPackageName);
    Account[] getRunningAccountsForSystem();
    Account[] getAllAccountsForSystemProcess();
}
