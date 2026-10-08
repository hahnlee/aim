// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

public class ApplicationInfo extends PackageItemInfo implements android.os.Parcelable {
    public String publicSourceDir;

    public void writeToParcel(android.os.Parcel out, int flags) { throw new RuntimeException("stub"); }
    public int describeContents() { throw new RuntimeException("stub"); }
    public static final android.os.Parcelable.Creator<ApplicationInfo> CREATOR = null;
    public long longVersionCode;
    public boolean isInstantApp() { throw new RuntimeException("stub"); }
    public boolean isSystemApp() { throw new RuntimeException("stub"); }
    public boolean enabled;
    public int targetSdkVersion;
    public int uid;
    public String sourceDir;
    public String[] splitNames;
    public String[] splitSourceDirs;
    public String[] resourceDirs;
    public String[] overlayPaths;
    public String[] sharedLibraryFiles;
    public java.util.List<SharedLibraryInfo> sharedLibraryInfos;
    public java.util.List<SharedLibraryInfo> optionalSharedLibraryInfos;
 public String getCodePath(){throw new RuntimeException("stub");}
}
