// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm.verify.domain;

import java.util.Map;

public final class DomainVerificationUserState implements android.os.Parcelable {
    public static final android.os.Parcelable.Creator<DomainVerificationUserState> CREATOR = null;
    @Override public void writeToParcel(android.os.Parcel out, int flags) { throw new RuntimeException("stub"); }
    @Override public int describeContents() { throw new RuntimeException("stub"); }
    public java.util.UUID getIdentifier() { throw new RuntimeException("stub"); }
    public String getPackageName() { throw new RuntimeException("stub"); }
    public android.os.UserHandle getUser() { throw new RuntimeException("stub"); }
    public DomainVerificationUserState(java.util.UUID domainSetId, String packageName, android.os.UserHandle user, boolean linkHandlingAllowed, Map<String, Integer> hostToStateMap) { throw new RuntimeException("stub"); }
    public boolean isLinkHandlingAllowed() { throw new RuntimeException("stub"); }
    public Map<String, Integer> getHostToStateMap() { throw new RuntimeException("stub"); }
}
