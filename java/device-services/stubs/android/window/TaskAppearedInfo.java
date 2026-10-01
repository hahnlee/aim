// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.window;

import android.app.ActivityManager;
import android.view.SurfaceControl;

public final class TaskAppearedInfo {
    public TaskAppearedInfo(ActivityManager.RunningTaskInfo taskInfo, SurfaceControl leash) { throw new RuntimeException("stub"); }
    public ActivityManager.RunningTaskInfo getTaskInfo() { throw new RuntimeException("stub"); }
    public SurfaceControl getLeash() { throw new RuntimeException("stub"); }
}
