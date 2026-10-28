package dev.aim.server;
final class ProviderQueriesTest {
    static void check(ProviderQueries queries) {
        var info = new android.content.pm.UserInfo();
        android.os.UserManager.info = info;
        info.clone = true;
        info.profileGroupId = 0;
        if (!queries.isCloneRedirected("media", 1010100, 0)
                || android.os.UserManager.requested != 10
                || queries.isCloneRedirected("media", 1010100, 11)
                || queries.isCloneRedirected("other", 1010100, 0)
                || queries.isCloneRedirected(null, 1010100, 0)) throw new AssertionError();
        info.clone = false;
        if (queries.isCloneRedirected("media", 1010100, 0)) throw new AssertionError();
        android.os.UserManager.info = null;
        if (queries.isCloneRedirected("media", 1010100, 0)) throw new AssertionError();
        Bridge.system = false;
        try { queries.isCloneRedirected("media", 1010100, 0); throw new AssertionError(); }
        catch (SecurityException expected) {}
        finally { Bridge.system = true; }
        var missing = new ProviderQueries(new android.content.Context() {
            public android.content.pm.PackageManager getPackageManager() { return null; }
            public <T> T getSystemService(Class<T> type) { return null; }
        });
        try { missing.isCloneRedirected("media", 1010100, 0); throw new AssertionError(); }
        catch (IllegalStateException expected) {}
    }
}
