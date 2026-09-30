// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.location;

import android.content.Context;

public class LocationManager {
    public static final String PROVIDERS_CHANGED_ACTION = "android.location.PROVIDERS_CHANGED";
    public static final String EXTRA_PROVIDER_NAME = "android.location.extra.PROVIDER_NAME";
    public static final String EXTRA_PROVIDER_ENABLED = "android.location.extra.PROVIDER_ENABLED";

    public LocationManager(Context context, ILocationManager service) { throw new RuntimeException("stub"); }
    public static void invalidateLocalLocationEnabledCaches() { throw new RuntimeException("stub"); }
}
