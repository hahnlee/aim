// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content;

import android.content.pm.PackageManager;

public abstract class Context {
    public abstract PackageManager getPackageManager();
    public abstract int checkPermission(String permission, int pid, int uid);
}
