// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content;

import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.content.res.Resources;
import android.os.Handler;
import android.os.UserHandle;

public abstract class Context {
    public static final String LOCATION_SERVICE = "location";

    public Resources getResources() { throw new RuntimeException("stub"); }
    public ApplicationInfo getApplicationInfo() { throw new RuntimeException("stub"); }
    public Intent registerReceiver(BroadcastReceiver receiver, IntentFilter filter) { throw new RuntimeException("stub"); }
    public Intent registerReceiverAsUser(BroadcastReceiver receiver, UserHandle user, IntentFilter filter, String broadcastPermission, Handler scheduler) { throw new RuntimeException("stub"); }
    public void unregisterReceiver(BroadcastReceiver receiver) { throw new RuntimeException("stub"); }
    public abstract PackageManager getPackageManager();
    public abstract int checkPermission(String permission, int pid, int uid);
}
