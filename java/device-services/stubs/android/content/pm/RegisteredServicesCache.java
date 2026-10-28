// Compile-only pinned image API, android-16.0.0_r1; checked by image linkage.
package android.content.pm;
public abstract class RegisteredServicesCache<V> {
    public RegisteredServicesCache(android.content.Context context, String action, String metadata, String attributes, XmlSerializerAndParser<V> serializer) { throw new RuntimeException("stub"); }
    public java.util.Collection<ServiceInfo<V>> getAllServices(int userId) { throw new RuntimeException("stub"); }
    public static class ServiceInfo<V> {
        public ServiceInfo(V type, ComponentInfo info, android.content.ComponentName component, long updated) { throw new RuntimeException("stub"); }
        public final V type = null;
    }
}
