package android.content;
import android.content.pm.RegisteredServicesCache.ServiceInfo;
public class SyncAdaptersCache {
    public static int constructed, user;
    public static boolean fail;
    public static java.util.List<ServiceInfo<SyncAdapterType>> services = new java.util.ArrayList<>();
    public SyncAdaptersCache(Context context) { constructed++; }
    public java.util.Collection<ServiceInfo<SyncAdapterType>> getAllServices(int userId) {
        user = userId;
        if (fail) throw new IllegalStateException("cache failure");
        return services;
    }
}
