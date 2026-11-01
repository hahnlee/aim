// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app;

public class ActivityOptions extends ComponentOptions {
    public static ActivityOptions makeBasic() { throw new RuntimeException("stub"); }
    public ActivityOptions setLaunchDisplayId(int id) { throw new RuntimeException("stub"); }
    public void setLaunchTaskDisplayAreaFeatureId(int id) { throw new RuntimeException("stub"); }
    public void setLaunchWindowingMode(int mode) { throw new RuntimeException("stub"); }
    public void setLaunchActivityType(int type) { throw new RuntimeException("stub"); }
    public void setLaunchTaskId(int id) { throw new RuntimeException("stub"); }
    public void setTaskOverlay(boolean overlay, boolean canResume) { throw new RuntimeException("stub"); }
    public ActivityOptions setLockTaskEnabled(boolean enabled) { throw new RuntimeException("stub"); }
    public ActivityOptions setSplashScreenStyle(int style) { throw new RuntimeException("stub"); }
    public void setDismissKeyguardIfInsecure() { throw new RuntimeException("stub"); }
    public android.os.Bundle toBundle() { throw new RuntimeException("stub"); }
    public static ActivityOptions fromBundle(android.os.Bundle bundle) { throw new RuntimeException("stub"); }
    public android.os.IRemoteCallback getAnimationStartedListener() { throw new RuntimeException("stub"); }
    public android.os.IRemoteCallback getAnimationFinishedListener() { throw new RuntimeException("stub"); }
    public int getLaunchWindowingMode() { throw new RuntimeException("stub"); }
}
