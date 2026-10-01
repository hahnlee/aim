// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm.verify.domain;

import android.content.pm.PackageManager.NameNotFoundException;

public final class DomainVerificationManager {
    public DomainVerificationManager(android.content.Context context, IDomainVerificationManager domainVerificationManager) { throw new RuntimeException("stub"); }
    public DomainVerificationInfo getDomainVerificationInfo(String packageName) throws NameNotFoundException { throw new RuntimeException("stub"); }
    public DomainVerificationUserState getDomainVerificationUserState(String packageName) throws NameNotFoundException { throw new RuntimeException("stub"); }
}
