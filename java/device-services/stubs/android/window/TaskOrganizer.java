// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.window;

import android.app.ActivityManager;
import android.view.SurfaceControl;

import java.util.List;
import java.util.concurrent.Executor;

public class TaskOrganizer extends WindowOrganizer {
    public TaskOrganizer(ITaskOrganizerController taskOrganizerController, Executor executor) { throw new RuntimeException("stub"); }
    public List<TaskAppearedInfo> registerOrganizer() { throw new RuntimeException("stub"); }
    public void onTaskAppeared(ActivityManager.RunningTaskInfo taskInfo, SurfaceControl leash) { throw new RuntimeException("stub"); }
    public void onTaskVanished(ActivityManager.RunningTaskInfo taskInfo) { throw new RuntimeException("stub"); }
    public void onTaskInfoChanged(ActivityManager.RunningTaskInfo taskInfo) { throw new RuntimeException("stub"); }
}
