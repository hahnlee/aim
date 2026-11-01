package com.android.server.content;
import android.accounts.Account;
import android.accounts.AccountAndUser;
public interface SyncAccounts {
    void validateAccounts(int userId);
    Account[] getAccounts(int userId, String opPackageName);
    AccountAndUser[] getRunningAccountsForSystem();
    AccountAndUser[] getAllAccountsForSystemProcess();
}
