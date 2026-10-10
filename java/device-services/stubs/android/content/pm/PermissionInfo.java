// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

public class PermissionInfo extends PackageItemInfo {
    public static final int PROTECTION_DANGEROUS = 1;
    public int descriptionRes;
    public CharSequence nonLocalizedDescription;
    public String group;
    public static final int PROTECTION_MASK_BASE=15;
    public static String protectionToString(int protection){throw new RuntimeException("stub");}

    public int protectionLevel;
    public PermissionInfo() { throw new RuntimeException("stub"); }
}
