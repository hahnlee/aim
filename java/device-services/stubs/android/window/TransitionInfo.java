// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.window;

import android.app.ActivityManager;

import java.util.List;

public final class TransitionInfo {
    public TransitionInfo(int type, int flags) { throw new RuntimeException("stub"); }
    public List<Change> getChanges() { throw new RuntimeException("stub"); }

    public static final class Change {
        public Change(WindowContainerToken container, android.view.SurfaceControl leash) { throw new RuntimeException("stub"); }
        public ActivityManager.RunningTaskInfo getTaskInfo() { throw new RuntimeException("stub"); }
        public int getMode() { throw new RuntimeException("stub"); }
    }
}
