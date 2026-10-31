package android.accounts;
public class AccountManager {
    public AccountManager(android.content.Context context, IAccountManager service, android.os.Handler handler) { throw new RuntimeException("stub"); }
    public Account[] getAccounts() { throw new RuntimeException("stub"); }
    public AccountManagerFuture<Boolean> hasFeatures(Account account, String[] features, AccountManagerCallback<Boolean> callback, android.os.Handler handler) { throw new RuntimeException("stub"); }
}
