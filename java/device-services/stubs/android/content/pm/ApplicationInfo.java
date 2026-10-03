// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

public class ApplicationInfo extends PackageItemInfo {
    public static final android.os.Parcelable.Creator<ApplicationInfo> CREATOR = null;
    public boolean enabled;
    public int targetSdkVersion;
    public int uid;
    public String sourceDir;
    public String[] splitSourceDirs;
    public String[] resourceDirs;
    public String[] overlayPaths;
    public String[] sharedLibraryFiles;
}
