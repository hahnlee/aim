// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content;

import android.content.pm.PackageManager;

public class ContextWrapper extends Context {
    public ContextWrapper(Context base) { throw new RuntimeException("stub"); }
    @Override
    public PackageManager getPackageManager() { throw new RuntimeException("stub"); }
    @Override
    public int checkPermission(String permission, int pid, int uid) { throw new RuntimeException("stub"); }
}
