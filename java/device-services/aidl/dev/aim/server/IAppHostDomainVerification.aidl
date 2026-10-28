package dev.aim.server;

import android.content.pm.verify.domain.DomainSet;
import android.content.pm.verify.domain.DomainVerificationInfo;
import android.content.UriRelativeFilterGroupParcel;
import android.os.Bundle;

/** Java policy/native state boundary; only the system UID may supply caller identity. */
interface IAppHostDomainVerification {
    List<String> queryValidVerificationPackageNames(int uid);
    DomainVerificationInfo getDomainVerificationInfo(int uid, String packageName);
    int setDomainVerificationStatus(int uid, String id, in DomainSet domains, int state);
    void setUriRelativeFilterGroups(int uid, String packageName, in Bundle groups);
    Bundle getUriRelativeFilterGroups(int uid, String packageName, in List<String> domains);
    String getDomainVerificationPackageId(String packageName);
    void setUriRelativeFilterGroup(String packageName, String id, String domain,
            in List<UriRelativeFilterGroupParcel> groups);
}
