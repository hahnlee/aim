// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content;

import android.os.IBinder;

public class Intent {
    public static final String EXTRA_PACKAGE_NAME = "android.intent.extra.PACKAGE_NAME";

    public Intent() { throw new RuntimeException("stub"); }
    public Intent(Intent o) { throw new RuntimeException("stub"); }
    public String getAction() { throw new RuntimeException("stub"); }
    public String getStringExtra(String name) { throw new RuntimeException("stub"); }
    public String[] getStringArrayExtra(String name) { throw new RuntimeException("stub"); }
    public IBinder getIBinderExtra(String name) { throw new RuntimeException("stub"); }
    public Intent setPackage(String packageName) { throw new RuntimeException("stub"); }
    public Intent setComponent(ComponentName component) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, IBinder value) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, String[] value) { throw new RuntimeException("stub"); }
    public Intent putExtra(String name, int[] value) { throw new RuntimeException("stub"); }
}
