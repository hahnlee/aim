// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

public class PackageItemInfo {
    public String name;
    public int icon;
    public CharSequence nonLocalizedLabel;
    public String packageName;
    public android.graphics.drawable.Drawable loadIcon(PackageManager pm) { throw new RuntimeException("stub"); }
    public CharSequence loadLabel(PackageManager pm) { throw new RuntimeException("stub"); }
}
