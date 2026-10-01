// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app;

import android.content.pm.ActivityInfo;
import android.window.WindowContainerToken;

public class TaskInfo {
    public int taskId;
    public int displayId;
    public int userId;
    public int topActivityType;
    public android.content.ComponentName topActivity;
    public WindowContainerToken token;
    public ActivityInfo topActivityInfo;
    public PictureInPictureParams pictureInPictureParams;
    public final android.content.res.Configuration configuration = null;
    public int getWindowingMode() { throw new RuntimeException("stub"); }
    public int getActivityType() { throw new RuntimeException("stub"); }
    public android.content.res.Configuration getConfiguration() { throw new RuntimeException("stub"); }
}
