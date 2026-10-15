// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app;

import android.content.Context;
import android.os.Handler;

public class ActivityManager {
    public int getLauncherLargeIconSize() { throw new RuntimeException("stub"); }
    public interface OnUidImportanceListener { void onUidImportance(int uid, int importance); }
    public void addOnUidImportanceListener(OnUidImportanceListener listener, int cutpoint) { throw new RuntimeException("stub"); }
    public void removeOnUidImportanceListener(OnUidImportanceListener listener) { throw new RuntimeException("stub"); }
    ActivityManager(Context context, Handler handler) { throw new RuntimeException("stub"); }

    public static class RunningTaskInfo extends TaskInfo {}
    public static IActivityManager getService() { throw new RuntimeException("stub"); }
 public static int handleIncomingUser(int pid,int uid,int user,boolean all,boolean full,String name,String caller){throw new RuntimeException("stub");}
}
