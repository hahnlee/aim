package android.content;
public abstract class Context {
    public static final String ROLE_SERVICE = "role";
    public static final String ACCESSIBILITY_SERVICE = "accessibility";
    public void sendOrderedBroadcastAsUser(Intent i, android.os.UserHandle user, String p, BroadcastReceiver receiver, android.os.Handler handler, int code, String data, android.os.Bundle extras) { receiver.onReceive(this, i); }
    public abstract android.content.pm.PackageManager getPackageManager();
    public <T> T getSystemService(Class<T> type) {
        return type.cast(new android.os.UserManager());
    }
}
