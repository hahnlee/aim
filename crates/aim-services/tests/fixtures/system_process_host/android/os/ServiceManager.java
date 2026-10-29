package android.os;
public final class ServiceManager {
    public static void addService(String name, IBinder service) {
        if (service == null) throw new AssertionError(name);
        if (name.equals("graphicsstats")) {
            if (!(service instanceof android.graphics.GraphicsStatsService)) throw new AssertionError(service);
            dev.aim.server.SystemProcessHostTest.events.add("graphicsstats:publish");
        } else if (name.equals("aim.role_queries")) {
            dev.aim.server.SystemProcessHostTest.events.add("roles");
        } else {
            throw new AssertionError(name);
        }
    }
    public static IBinder getService(String name) { throw new AssertionError(); }
    public static IBinder waitForService(String name) {
        if (!name.equals("aim.system_process")) throw new AssertionError(name);
        return null;
    }
}
