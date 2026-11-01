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
        failure[0] = 3;
        for (int i = 1; i < methods.length; i++) {
            try { methods[i].run(); throw new AssertionError("null snapshot accepted"); }
            catch (NullPointerException expected) { }
        }
    }
    private static void check(AccountAndUser[] result, Account[] accounts) {
        if (result.length != 2) throw new AssertionError();
        for (int i = 0; i < result.length; i++) if (result[i].userId != 0 || result[i].account != accounts[i]) throw new AssertionError();
    }
}
