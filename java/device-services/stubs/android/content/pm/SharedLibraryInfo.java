// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

import java.util.List;

public final class SharedLibraryInfo implements android.os.Parcelable {
    public static final android.os.Parcelable.Creator<SharedLibraryInfo> CREATOR = null;
    public void writeToParcel(android.os.Parcel out, int flags) { throw new RuntimeException("stub"); }
    public int describeContents() { throw new RuntimeException("stub"); }
    public SharedLibraryInfo(String name, long version, int type) { throw new RuntimeException("stub"); }
    public SharedLibraryInfo(String path, String packageName, List<String> codePaths, String name,
            long version, int type, VersionedPackage declaring, List<VersionedPackage> dependents,
            List<SharedLibraryInfo> dependencies, boolean nativeLibrary) { throw new RuntimeException("stub"); }
    public SharedLibraryInfo(String name, long version, int type, List<String> certDigests) { throw new RuntimeException("stub"); }
    public String getName() { throw new RuntimeException("stub"); }
    public String getPath() { throw new RuntimeException("stub"); }
    public String getPackageName() { throw new RuntimeException("stub"); }
    public List<String> getAllCodePaths() { throw new RuntimeException("stub"); }
    public long getLongVersion() { throw new RuntimeException("stub"); }
    public int getType() { throw new RuntimeException("stub"); }
    public boolean isNative() { throw new RuntimeException("stub"); }
    public VersionedPackage getDeclaringPackage() { throw new RuntimeException("stub"); }
    public List<VersionedPackage> getDependentPackages() { throw new RuntimeException("stub"); }
    public List<VersionedPackage> getOptionalDependentPackages() { throw new RuntimeException("stub"); }
    public List<String> getCertDigests() { throw new RuntimeException("stub"); }
    public List<SharedLibraryInfo> getDependencies() { throw new RuntimeException("stub"); }
}
