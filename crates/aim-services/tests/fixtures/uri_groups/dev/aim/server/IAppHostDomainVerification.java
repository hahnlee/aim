package dev.aim.server;
import android.content.UriRelativeFilterGroupParcel;
import android.os.Bundle;
import java.util.List;
/** Only the backend operations used by the Java policy under test. */
public interface IAppHostDomainVerification {
    String getDomainVerificationPackageId(String name);
    void setUriRelativeFilterGroup(String name, String id, String domain, List<UriRelativeFilterGroupParcel> groups);
    Bundle getUriRelativeFilterGroups(int uid, String name, List<String> domains);
}
