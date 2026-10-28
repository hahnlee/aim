package android.os;
public final class ServiceManager {
    public static void addService(String name, IBinder service) {
        if (!name.equals("aim.role_queries") || service == null) throw new AssertionError(name);
        dev.aim.server.SystemProcessHostTest.events.add("roles");
    }
    public static IBinder getService(String name) { throw new AssertionError(); }
    public static IBinder waitForService(String name) {
        if (!name.equals("aim.system_process")) throw new AssertionError(name);
        return null;
    }
}
