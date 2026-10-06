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
    public static final String PLATFORM_COMPAT_SERVICE = "platform_compat";
    public static final String VR_SERVICE = "vrmanager";
    public static final int CONTEXT_RESTRICTED = 4;

    public Resources getResources() { throw new RuntimeException("stub"); }
    public abstract void setTheme(int resid);
    public abstract Resources.Theme getTheme();
    public final android.content.res.TypedArray obtainStyledAttributes(int[] attrs) { throw new RuntimeException("stub"); }
    public Context createPackageContextAsUser(String packageName, int flags, UserHandle user) throws PackageManager.NameNotFoundException { throw new RuntimeException("stub"); }
    public abstract Context createConfigurationContext(android.content.res.Configuration overrideConfiguration);
    public ApplicationInfo getApplicationInfo() { throw new RuntimeException("stub"); }
    public Context createContextAsUser(UserHandle user, int flags) { throw new RuntimeException("stub"); }
    public Intent registerReceiver(BroadcastReceiver receiver, IntentFilter filter) { throw new RuntimeException("stub"); }
    public Intent registerReceiverAsUser(BroadcastReceiver receiver, UserHandle user, IntentFilter filter, String broadcastPermission, Handler scheduler) { throw new RuntimeException("stub"); }
    public void unregisterReceiver(BroadcastReceiver receiver) { throw new RuntimeException("stub"); }
    public abstract PackageManager getPackageManager();
    public abstract ContentResolver getContentResolver();
    public final <T> T getSystemService(Class<T> serviceClass) { throw new RuntimeException("stub"); }
    public abstract int checkPermission(String permission, int pid, int uid);
    public abstract void enforcePermission(String permission, int pid, int uid, String message);
    public final int getColor(int id) { throw new RuntimeException("stub"); }
    public final String getString(int resId) { throw new RuntimeException("stub"); }
    public abstract String getBasePackageName();
    public String getAttributionTag() { throw new RuntimeException("stub"); }
    public abstract void sendBroadcastAsUser(Intent intent, UserHandle user);
    public abstract void sendBroadcastAsUser(Intent intent, UserHandle user, String receiverPermission);
    public abstract void sendOrderedBroadcastAsUser(Intent intent, UserHandle user, String receiverPermission, BroadcastReceiver resultReceiver, Handler scheduler, int initialCode, String initialData, android.os.Bundle initialExtras);
}
