package android.app;
import dev.aim.server.SystemProcessHostTest;
public final class ActivityThread {
    public static ActivityThread systemMain() {
        SystemProcessHostTest.events.add("context");
        return new ActivityThread();
    }
    public android.content.Context getSystemContext() { return null; }
    public static void initializeMainlineModules() {
        SystemProcessHostTest.events.add("modules");
    }
    public IApplicationThread getApplicationThread() { return () -> SystemProcessHostTest.applicationThread; }
}
