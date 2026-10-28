// Compile-only original image type; no runtime implementation.
package android.content.pm;
public class ServiceInfo extends ComponentInfo { public static int FOREGROUND_SERVICE_TYPE_SYSTEM_EXEMPTED, FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION,
    FOREGROUND_SERVICE_TYPE_MANIFEST, FOREGROUND_SERVICE_TYPE_NONE,
    FOREGROUND_SERVICE_TYPE_MEDIA_PROCESSING, FOREGROUND_SERVICE_TYPE_DATA_SYNC;
    public static String foregroundServiceTypeToLabel(int type) { throw new RuntimeException("stub"); }
    private ServiceInfo() { throw new RuntimeException("stub"); } }
