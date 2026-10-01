// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.window;

import android.app.ActivityManager;
import android.content.pm.ActivityInfo;
import android.os.IBinder;

public final class StartingWindowInfo {
    public static final int STARTING_WINDOW_TYPE_NONE = 0;
    public static final int STARTING_WINDOW_TYPE_SPLASH_SCREEN = 1;
    public static final int STARTING_WINDOW_TYPE_SOLID_COLOR_SPLASH_SCREEN = 3;
    public static final int STARTING_WINDOW_TYPE_LEGACY_SPLASH_SCREEN = 4;
    public static final int TYPE_PARAMETER_NEW_TASK = 1;
    public static final int TYPE_PARAMETER_TASK_SWITCH = 2;
    public static final int TYPE_PARAMETER_PROCESS_RUNNING = 4;
    public static final int TYPE_PARAMETER_ALLOW_TASK_SNAPSHOT = 8;
    public static final int TYPE_PARAMETER_ACTIVITY_CREATED = 16;
    public static final int TYPE_PARAMETER_USE_SOLID_COLOR_SPLASH_SCREEN = 32;
    public static final int TYPE_PARAMETER_ACTIVITY_DRAWN = 64;
    public static final int TYPE_PARAMETER_WINDOWLESS = 256;
    public static final int TYPE_PARAMETER_LEGACY_SPLASH_SCREEN = -2147483648;
    public ActivityManager.RunningTaskInfo taskInfo;
    public ActivityInfo targetActivityInfo;
    public int startingWindowTypeParameter;
    public int splashScreenThemeResId;
    public boolean isKeyguardOccluded;
    public TaskSnapshot taskSnapshot;
    public IBinder appToken;
    public boolean allowHandleSolidColorSplashScreen() { throw new RuntimeException("stub"); }
}
