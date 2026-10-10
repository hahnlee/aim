// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content;

import android.content.pm.PackageManager;

public class ContextWrapper extends Context {
    public ContextWrapper(Context base) { throw new RuntimeException("stub"); }
    @Override
    public void setTheme(int resid) { throw new RuntimeException("stub"); }
    @Override
    public android.content.res.Resources.Theme getTheme() { throw new RuntimeException("stub"); }
    @Override
    public Context createConfigurationContext(android.content.res.Configuration overrideConfiguration) { throw new RuntimeException("stub"); }
    @Override
    public PackageManager getPackageManager() { throw new RuntimeException("stub"); }
    @Override
    public ContentResolver getContentResolver() { throw new RuntimeException("stub"); }
    @Override
    public int checkPermission(String permission, int pid, int uid) { throw new RuntimeException("stub"); }
    @Override
    public void enforcePermission(String permission, int pid, int uid, String message) { throw new RuntimeException("stub"); }
    @Override
    public void startActivityAsUser(Intent intent, android.os.UserHandle user) { throw new RuntimeException("stub"); }
    public String getBasePackageName() { throw new RuntimeException("stub"); }
    @Override
    public void sendBroadcastAsUser(Intent intent, android.os.UserHandle user) { throw new RuntimeException("stub"); }
    @Override
    public void sendBroadcastAsUser(Intent intent, android.os.UserHandle user, String receiverPermission) { throw new RuntimeException("stub"); }
    @Override
    public void sendOrderedBroadcastAsUser(Intent intent, android.os.UserHandle user, String receiverPermission, BroadcastReceiver resultReceiver, android.os.Handler scheduler, int initialCode, String initialData, android.os.Bundle initialExtras) { throw new RuntimeException("stub"); }
}
