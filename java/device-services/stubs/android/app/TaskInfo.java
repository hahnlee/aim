// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app;

import android.content.pm.ActivityInfo;
import android.window.WindowContainerToken;

public class TaskInfo {
    public int taskId;
    public int displayId;
    public WindowContainerToken token;
    public ActivityInfo topActivityInfo;
    public int getWindowingMode() { throw new RuntimeException("stub"); }
    public int getActivityType() { throw new RuntimeException("stub"); }
}
