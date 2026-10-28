package android.app;
import dev.aim.server.SystemProcessHostTest;
public final class ActivityThread {
    public static final android.content.Context context = new android.content.Context() {
        public android.content.pm.PackageManager getPackageManager() {
            return new android.content.pm.PackageManager();
        }
    };
    public static ActivityThread systemMain() {
        SystemProcessHostTest.events.add("context");
        return new ActivityThread();
    }
    public android.content.Context getSystemContext() { return context; }
    public static void initializeMainlineModules() {
        SystemProcessHostTest.events.add("modules");
    }
    public void installSystemApplicationInfo(android.content.pm.ApplicationInfo info, ClassLoader loader) {
        if (loader != SystemProcessHostTest.class.getClassLoader()) throw new AssertionError("loader");
        SystemProcessHostTest.events.add("systemApplication");
    }
    public IApplicationThread getApplicationThread() { return () -> SystemProcessHostTest.applicationThread; }
}
