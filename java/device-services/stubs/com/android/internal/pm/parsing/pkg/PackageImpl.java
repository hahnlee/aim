// Compile-only image API; checked by the device-services build node.
package com.android.internal.pm.parsing.pkg;
import com.android.internal.pm.pkg.component.*;
import com.android.internal.pm.pkg.parsing.ParsingPackage;
public abstract class PackageImpl implements ParsedPackage {
    public PackageImpl(android.os.Parcel in) { throw new RuntimeException("stub"); }
    public static ParsingPackage forTesting(String name) { throw new RuntimeException("stub"); }
    public PackageImpl setPackageName(String name) { throw new RuntimeException("stub"); }
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
