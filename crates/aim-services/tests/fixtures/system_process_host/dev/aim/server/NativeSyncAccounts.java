package dev.aim.server;
final class NativeSyncAccounts implements com.android.server.content.SyncAccounts {
    NativeSyncAccounts(Object owner) { if (owner == null) throw new AssertionError(); }
    public void validateAccounts(int user) { throw new AssertionError(); }
    public android.accounts.Account[] getAccounts(int user, String pkg) { throw new AssertionError(); }
    public android.accounts.AccountAndUser[] getRunningAccountsForSystem() { throw new AssertionError(); }
    public android.accounts.AccountAndUser[] getAllAccountsForSystemProcess() { throw new AssertionError(); }
}
