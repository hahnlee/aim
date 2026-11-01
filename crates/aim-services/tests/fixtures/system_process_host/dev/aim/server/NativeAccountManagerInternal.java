package dev.aim.server;
final class NativeAccountManagerInternal extends android.accounts.AccountManagerInternal {
    NativeAccountManagerInternal(android.content.Context context, Object owner) {}
    public boolean hasAccountAccess(android.accounts.Account a, int uid) { throw new AssertionError(); }
    public void requestAccountAccess(android.accounts.Account a, String pkg, int user, android.os.RemoteCallback cb) { throw new AssertionError(); }
    public void addOnAppPermissionChangeListener(OnAppPermissionChangeListener listener) { throw new AssertionError(); }
    public byte[] backupAccountAccessPermissions(int user) { throw new AssertionError(); }
    public void restoreAccountAccessPermissions(byte[] data, int user) { throw new AssertionError(); }
}
