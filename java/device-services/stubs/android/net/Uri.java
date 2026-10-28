// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.net;

public abstract class Uri implements android.os.Parcelable {
    public abstract String getScheme();
    public static final android.os.Parcelable.Creator<Uri> CREATOR = null;
    public static Uri fromParts(String scheme, String ssp, String fragment) { throw new RuntimeException("stub"); }
    public static Uri parse(String uriString) { throw new RuntimeException("stub"); }
    public abstract String getSchemeSpecificPart();
}
