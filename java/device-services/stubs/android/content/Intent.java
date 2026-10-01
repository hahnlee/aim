// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content;

import android.os.IBinder;

public class Intent {
    public static final String ACTION_SCREEN_OFF = "android.intent.action.SCREEN_OFF";
    public static final String ACTION_SCREEN_ON = "android.intent.action.SCREEN_ON";
    public static final String ACTION_PACKAGE_CHANGED = "android.intent.action.PACKAGE_CHANGED";
    public static final String ACTION_PACKAGE_REMOVED = "android.intent.action.PACKAGE_REMOVED";
    public static final String ACTION_PACKAGE_RESTARTED = "android.intent.action.PACKAGE_RESTARTED";
    public static final String ACTION_QUERY_PACKAGE_RESTART = "android.intent.action.QUERY_PACKAGE_RESTART";
    public static final String EXTRA_PACKAGES = "android.intent.extra.PACKAGES";
    public static final String EXTRA_CHANGED_COMPONENT_NAME_LIST = "android.intent.extra.changed_component_name_list";
    public static final String EXTRA_PACKAGE_NAME = "android.intent.extra.PACKAGE_NAME";
    public static final String ACTION_MAIN = "android.intent.action.MAIN";
    public static final String ACTION_DOCK_EVENT = "android.intent.action.DOCK_EVENT";
    public static final String ACTION_BATTERY_CHANGED = "android.intent.action.BATTERY_CHANGED";
    public static final String ACTION_SETTING_RESTORED = "android.os.action.SETTING_RESTORED";
    public static final String ACTION_SHUTDOWN = "android.intent.action.ACTION_SHUTDOWN";
    public static final String ACTION_DREAMING_STARTED = "android.intent.action.DREAMING_STARTED";
    public static final String ACTION_TIME_CHANGED = "android.intent.action.TIME_SET";
    public static final String ACTION_TIMEZONE_CHANGED = "android.intent.action.TIMEZONE_CHANGED";
    public static final String EXTRA_DOCK_STATE = "android.intent.extra.DOCK_STATE";
    public static final int EXTRA_DOCK_STATE_UNDOCKED = 0;
    public static final String EXTRA_SETTING_NAME = "setting_name";
    public static final int FLAG_RECEIVER_FOREGROUND = 0x10000000;
    public static final int FLAG_ACTIVITY_NEW_TASK = 0x10000000;
    public static final int FLAG_ACTIVITY_RESET_TASK_IF_NEEDED = 0x00200000;

    public Intent() { throw new RuntimeException("stub"); }
    public Intent(Intent o) { throw new RuntimeException("stub"); }
    public Intent(String action) { throw new RuntimeException("stub"); }
    public int getIntExtra(String name, int defaultValue) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, int value) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, String value) { throw new RuntimeException("stub"); }
    public Intent addFlags(int flags) { throw new RuntimeException("stub"); }
    public Intent setFlags(int flags) { throw new RuntimeException("stub"); }
    public Intent addCategory(String category) { throw new RuntimeException("stub"); }
    public Intent setClassName(String packageName, String className) { throw new RuntimeException("stub"); }
    public String getAction() { throw new RuntimeException("stub"); }
    public String getStringExtra(String name) { throw new RuntimeException("stub"); }
    public boolean getBooleanExtra(String name, boolean defaultValue) { throw new RuntimeException("stub"); }
    public boolean hasExtra(String name) { throw new RuntimeException("stub"); }
    public <T extends android.os.Parcelable> T getParcelableExtra(String name) { throw new RuntimeException("stub"); }
    public Intent putExtras(android.os.Bundle extras) { throw new RuntimeException("stub"); }
    public android.net.Uri getData() { throw new RuntimeException("stub"); }
    public String[] getStringArrayExtra(String name) { throw new RuntimeException("stub"); }
    public IBinder getIBinderExtra(String name) { throw new RuntimeException("stub"); }
    public Intent setPackage(String packageName) { throw new RuntimeException("stub"); }
    public Intent setComponent(ComponentName component) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, IBinder value) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, String[] value) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, int[] value) { throw new RuntimeException("stub"); }
}
