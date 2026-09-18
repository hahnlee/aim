package android.os;

public final class ServiceManager {
    private static IBinder service;
    public static IBinder getService(String name) { return service; }
    public static void setService(IBinder value) { service = value; }
}
