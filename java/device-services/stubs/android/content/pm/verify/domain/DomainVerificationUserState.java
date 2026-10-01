// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm.verify.domain;

import java.util.Map;

public final class DomainVerificationUserState {
    public DomainVerificationUserState(java.util.UUID domainSetId, String packageName, android.os.UserHandle user, boolean linkHandlingAllowed, Map<String, Integer> hostToStateMap) { throw new RuntimeException("stub"); }
    public boolean isLinkHandlingAllowed() { throw new RuntimeException("stub"); }
    public Map<String, Integer> getHostToStateMap() { throw new RuntimeException("stub"); }
}
