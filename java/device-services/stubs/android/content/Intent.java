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

    public Intent() { throw new RuntimeException("stub"); }
    public Intent(Intent o) { throw new RuntimeException("stub"); }
    public String getAction() { throw new RuntimeException("stub"); }
    public String getStringExtra(String name) { throw new RuntimeException("stub"); }
    public boolean getBooleanExtra(String name, boolean defaultValue) { throw new RuntimeException("stub"); }
    public android.net.Uri getData() { throw new RuntimeException("stub"); }
    public String[] getStringArrayExtra(String name) { throw new RuntimeException("stub"); }
    public IBinder getIBinderExtra(String name) { throw new RuntimeException("stub"); }
    public Intent setPackage(String packageName) { throw new RuntimeException("stub"); }
    public Intent setComponent(ComponentName component) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, IBinder value) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, String[] value) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, int[] value) { throw new RuntimeException("stub"); }
}
