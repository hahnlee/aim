package dev.aim.server;
import android.accounts.*;
import android.os.RemoteException;
import java.lang.reflect.Proxy;
import java.util.*;
public final class SyncAccountsTest {
    public static void main(String[] args) {
        List<String> calls = new ArrayList<>();
        Account[] accounts = {new Account("a", "first"), new Account("b", "second")};
        int[] failure = {0};
        ISyncAccounts owner = (ISyncAccounts) Proxy.newProxyInstance(ISyncAccounts.class.getClassLoader(),
                new Class<?>[]{ISyncAccounts.class}, (proxy, method, values) -> {
                    calls.add(method.getName() + Arrays.toString(values));
                    if (failure[0] == 1) throw new IllegalStateException("owner");
                    if (failure[0] == 2) throw new RemoteException();
                    if (failure[0] == 3) return null;
                    return method.getName().equals("validateAccounts") ? null : accounts;
                });
        NativeSyncAccounts adapter = new NativeSyncAccounts(owner);
        Runnable[] methods = {() -> adapter.validateAccounts(7), () -> {
            if (adapter.getAccounts(7, "android") != accounts) throw new AssertionError();
        }, () -> check(adapter.getRunningAccountsForSystem(), accounts), () -> check(adapter.getAllAccountsForSystemProcess(), accounts)};
        for (Runnable method : methods) method.run();
        if (!calls.equals(List.of("validateAccounts[7]", "getAccounts[7, android]",
                "getRunningAccountsForSystemnull", "getAllAccountsForSystemProcessnull"))) throw new AssertionError(calls);
        for (int mode : new int[]{1, 2}) {
            failure[0] = mode;
            for (Runnable method : methods) {
                try { method.run(); throw new AssertionError("failure swallowed"); }
                catch (IllegalStateException expected) { if (mode == 2 && !(expected.getCause() instanceof RemoteException)) throw expected; }
            }
        }
        checkInternal(accounts[0]);
        failure[0] = 3;
        for (int i = 1; i < methods.length; i++) {
            try { methods[i].run(); throw new AssertionError("null snapshot accepted"); }
            catch (NullPointerException expected) { }
        }
    }
    private static void checkInternal(Account account) {
        List<String> calls = new ArrayList<>();
        IAccountPermissionListener[] registered = {null};
        int[] failure = {0};
        ISyncAccounts owner = (ISyncAccounts) Proxy.newProxyInstance(ISyncAccounts.class.getClassLoader(),
                new Class<?>[]{ISyncAccounts.class}, (proxy, method, values) -> {
                    if (failure[0] == 1) throw new IllegalStateException("database");
                    if (failure[0] == 2) throw new RemoteException();
                    calls.add(method.getName());
                    if (method.getName().equals("hasAccountAccess")) {
                        if (values[0] != account || (int) values[1] != 10101) throw new AssertionError();
                        return true;
                    }
                    registered[0] = (IAccountPermissionListener) values[0];
                    return null;
                });
        NativeAccountManagerInternal adapter = new NativeAccountManagerInternal(owner);
        int[] events = {0};
        AccountManagerInternal.OnAppPermissionChangeListener listener = (changed, uid) -> {
            if (changed != account || uid != 10101 || !adapter.hasAccountAccess(changed, uid)) throw new AssertionError();
            events[0]++;
        };
        if (!adapter.hasAccountAccess(account, 10101)) throw new AssertionError();
        adapter.addOnAppPermissionChangeListener(listener);
        try { registered[0].onAppPermissionChanged(account, 10101); }
        catch (RemoteException e) { throw new AssertionError(e); }
        if (events[0] != 1 || !calls.equals(List.of("hasAccountAccess", "addOnAppPermissionChangeListener", "hasAccountAccess"))) throw new AssertionError(calls);
        for (int mode : new int[]{1, 2}) {
            failure[0] = mode;
            for (Runnable call : new Runnable[]{() -> adapter.hasAccountAccess(account, 10101), () -> adapter.addOnAppPermissionChangeListener(listener)}) {
                try { call.run(); throw new AssertionError("failure swallowed"); }
                catch (IllegalStateException expected) { if (mode == 2 && !(expected.getCause() instanceof RemoteException)) throw expected; }
            }
        }
        for (Runnable call : new Runnable[]{() -> adapter.requestAccountAccess(account, "q", 0, null), () -> adapter.backupAccountAccessPermissions(0), () -> adapter.restoreAccountAccessPermissions(new byte[0], 0)}) {
            try { call.run(); throw new AssertionError("inactive operation succeeded"); }
            catch (UnsupportedOperationException expected) { }
        }
    }
    private static void check(AccountAndUser[] result, Account[] accounts) {
        if (result.length != 2) throw new AssertionError();
        for (int i = 0; i < result.length; i++) if (result[i].userId != 0 || result[i].account != accounts[i]) throw new AssertionError();
    }
}
