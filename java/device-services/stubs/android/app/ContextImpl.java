// Compile-only return type checked against the pinned original image.
package android.app;
import android.content.*;
import android.content.pm.PackageManager;
import android.content.res.*;
import android.os.*;
final class ContextImpl extends Context {
    @Override public void startActivityAsUser(android.content.Intent intent, android.os.UserHandle user) { throw new RuntimeException("stub"); }
    private ContextImpl(ContextImpl container, ActivityThread thread, LoadedApk packageInfo, android.content.ContextParams params, String splitName, android.content.AttributionSource attribution, String feature, android.os.IBinder token, android.os.UserHandle user, int flags, ClassLoader loader, String device, int deviceId, boolean ui) { throw new RuntimeException("stub"); }
    public void setTheme(int resid) { throw new RuntimeException("stub"); }
    public Resources.Theme getTheme() { throw new RuntimeException("stub"); }
    public Context createConfigurationContext(android.content.res.Configuration overrideConfiguration) { throw new RuntimeException("stub"); }
    public PackageManager getPackageManager() { throw new RuntimeException("stub"); }
    public ContentResolver getContentResolver() { throw new RuntimeException("stub"); }
    public int checkPermission(String permission, int pid, int uid) { throw new RuntimeException("stub"); }
    public void enforcePermission(String permission, int pid, int uid, String message) { throw new RuntimeException("stub"); }
    public String getBasePackageName() { throw new RuntimeException("stub"); }
    public void sendBroadcastAsUser(Intent intent, UserHandle user) { throw new RuntimeException("stub"); }
    public void sendBroadcastAsUser(Intent intent, UserHandle user, String receiverPermission) { throw new RuntimeException("stub"); }
    public void sendOrderedBroadcastAsUser(Intent intent, UserHandle user, String receiverPermission, BroadcastReceiver resultReceiver, Handler scheduler, int initialCode, String initialData, android.os.Bundle initialExtras) { throw new RuntimeException("stub"); }
}
