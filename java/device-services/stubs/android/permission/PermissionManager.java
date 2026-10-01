// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.permission;

import java.util.Map;

public final class PermissionManager {
    public PermissionManager(android.content.Context context) { throw new RuntimeException("stub"); }
    public Map<String, PermissionState> getAllPermissionStates(String packageName, String persistentDeviceId) { throw new RuntimeException("stub"); }

    public static final class PermissionState {
        public PermissionState(boolean granted, int flags) { throw new RuntimeException("stub"); }
        public boolean isGranted() { throw new RuntimeException("stub"); }
    }
}
