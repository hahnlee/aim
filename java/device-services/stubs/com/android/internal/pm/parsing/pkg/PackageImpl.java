// Compile-only image API; checked by the device-services build node.
package com.android.internal.pm.parsing.pkg;
import com.android.internal.pm.pkg.component.*;
import com.android.internal.pm.pkg.parsing.ParsingPackage;
public abstract class PackageImpl implements ParsedPackage {
    public PackageImpl(android.os.Parcel in) { throw new RuntimeException("stub"); }
    public static ParsingPackage forTesting(String name) { throw new RuntimeException("stub"); }
    public PackageImpl setPackageName(String name) { throw new RuntimeException("stub"); }
    public PackageImpl setPath(String path) { throw new RuntimeException("stub"); }
    public PackageImpl setBaseApkPath(String path) { throw new RuntimeException("stub"); }
    public PackageImpl setPrimaryCpuAbi(String abi) { throw new RuntimeException("stub"); }
    public PackageImpl setSecondaryCpuAbi(String abi) { throw new RuntimeException("stub"); }
    public PackageImpl setSplitCodePaths(String[] paths) { throw new RuntimeException("stub"); }
    public PackageImpl setMultiArch(boolean value) { throw new RuntimeException("stub"); }
    public PackageImpl set32BitAbiPreferred(boolean value) { throw new RuntimeException("stub"); }
    public PackageImpl setFactoryTest(boolean value) { throw new RuntimeException("stub"); }
    public boolean isFactoryTest() { throw new RuntimeException("stub"); }
    public java.util.Set<String> getRequestedPermissions() { throw new RuntimeException("stub"); }
    public PackageImpl addImplicitPermission(String permission) { throw new RuntimeException("stub"); }
    public PackageImpl addLibraryName(String name) { throw new RuntimeException("stub"); }
    public String getNativeLibraryRootDir() { throw new RuntimeException("stub"); }
    public boolean isNativeLibraryRootRequiresIsa() { throw new RuntimeException("stub"); }
    public String getNativeLibraryDir() { throw new RuntimeException("stub"); }
    public String getSecondaryNativeLibraryDir() { throw new RuntimeException("stub"); }
    public PackageImpl setTargetSdkVersion(int value) { throw new RuntimeException("stub"); }
    public PackageImpl addUsesLibrary(String name) { throw new RuntimeException("stub"); }
    public PackageImpl addUsesOptionalLibrary(String name) { throw new RuntimeException("stub"); }
    public String getPackageName() { throw new RuntimeException("stub"); }
    public String getManifestPackageName() { throw new RuntimeException("stub"); }
    public PackageImpl addActivity(ParsedActivity component) { throw new RuntimeException("stub"); }
    public PackageImpl addReceiver(ParsedActivity component) { throw new RuntimeException("stub"); }
    public PackageImpl addService(ParsedService component) { throw new RuntimeException("stub"); }
    public PackageImpl addProvider(ParsedProvider component) { throw new RuntimeException("stub"); }
    public PackageImpl addPermission(ParsedPermission component) { throw new RuntimeException("stub"); }
    public PackageImpl addPermissionGroup(ParsedPermissionGroup component) { throw new RuntimeException("stub"); }
    public PackageImpl addInstrumentation(ParsedInstrumentation component) { throw new RuntimeException("stub"); }
}
