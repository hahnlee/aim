package android.os;

/** Hidden framework compile signature only; never included in the runtime DEX. */
public final class ServiceManager {
    private ServiceManager() {}
    public static native IBinder getService(String name);
}
