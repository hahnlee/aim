package android.content.pm;
public class RegisteredServicesCache<V> {
    public static class ServiceInfo<V> {
        public final V type;
        public ServiceInfo(V type) { this.type = type; }
    }
}
