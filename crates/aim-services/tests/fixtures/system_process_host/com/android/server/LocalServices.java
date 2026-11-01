package com.android.server;
public final class LocalServices {
    public static <T> void addService(Class<T> type, T service) {
        if (type != android.accounts.AccountManagerInternal.class || service == null) throw new AssertionError();
        dev.aim.server.SystemProcessHostTest.events.add("accountLocal");
    }
}
