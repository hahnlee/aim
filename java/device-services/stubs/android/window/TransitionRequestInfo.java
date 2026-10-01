// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.window;

import android.app.ActivityManager;
import android.os.Parcel;

public final class TransitionRequestInfo {
    TransitionRequestInfo(Parcel in) { throw new RuntimeException("stub"); }
    public int getType() { throw new RuntimeException("stub"); }
    public ActivityManager.RunningTaskInfo getTriggerTask() { throw new RuntimeException("stub"); }
}
