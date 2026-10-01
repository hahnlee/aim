// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm.verify.domain;

import java.util.Map;
import java.util.UUID;

public final class DomainVerificationInfo {
    public DomainVerificationInfo(UUID identifier, String packageName, Map<String, Integer> hostToStateMap) { throw new RuntimeException("stub"); }
    public UUID getIdentifier() { throw new RuntimeException("stub"); }
    public Map<String, Integer> getHostToStateMap() { throw new RuntimeException("stub"); }
}
