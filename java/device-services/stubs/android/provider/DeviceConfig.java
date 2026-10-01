// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.provider;

public final class DeviceConfig {
    public static final String NAMESPACE_PACKAGE_MANAGER_SERVICE = "package_manager_service";

    public static boolean getBoolean(String namespace, String name, boolean defaultValue) { throw new RuntimeException("stub"); }
    public static void addOnPropertiesChangedListener(String namespace,
            java.util.concurrent.Executor executor, OnPropertiesChangedListener listener) { throw new RuntimeException("stub"); }

    public interface OnPropertiesChangedListener {
        void onPropertiesChanged(Properties properties);
    }

    public static class Properties {
        public Properties(String namespace, java.util.Map<String, String> keyValueMap) { throw new RuntimeException("stub"); }
    }
}
