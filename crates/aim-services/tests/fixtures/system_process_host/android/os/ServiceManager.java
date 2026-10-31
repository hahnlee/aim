package android.os;
public final class ServiceManager {
    private static boolean safetyCenterPublished;
    public static void addService(String name, IBinder service) {
        if (service == null) throw new AssertionError(name);
        if (name.equals("graphicsstats")) {
            if (!(service instanceof android.graphics.GraphicsStatsService)) throw new AssertionError(service);
            dev.aim.server.SystemProcessHostTest.events.add("graphicsstats:publish");
        } else if (name.equals("aim.role_queries")) {
            dev.aim.server.SystemProcessHostTest.events.add("roles");
        } else if (name.equals("safety_center")) {
            if (safetyCenterPublished) throw new AssertionError("duplicate safety_center");
            safetyCenterPublished = true;
            dev.aim.server.SystemProcessHostTest.events.add("safety_center:publish");
        } else {
            throw new AssertionError(name);
        }
    }
    public static void requireSafetyCenter() { if (!safetyCenterPublished) throw new AssertionError("Safety Center must precede accessibility phase 600"); }
    public static IBinder getService(String name) { if (!name.equals("accessibility")) throw new AssertionError(name); return null; }
    public static IBinder waitForService(String name) {
        if (!name.equals("aim.system_process")) throw new AssertionError(name);
        return null;
    }
}
