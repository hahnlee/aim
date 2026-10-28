// Host-only Android boundary double; production policy is compiled unchanged.
package android.content;
public abstract class Context {
    public static final String USER_SERVICE="user";
    public java.util.concurrent.Executor getMainExecutor() { return task -> android.os.Handler.tasks.add(task); }
    public abstract Object getSystemService(String name);
    public abstract android.content.pm.PackageManager getPackageManager();
    public abstract Intent registerReceiverForAllUsers(BroadcastReceiver receiver,
        IntentFilter filter,String permission,android.os.Handler handler);
    public abstract void unregisterReceiver(BroadcastReceiver receiver);
}
