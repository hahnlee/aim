package dev.aim.server;

import android.content.pm.verify.domain.DomainSet;
import android.content.pm.verify.domain.DomainVerificationInfo;

/** Java policy/native state boundary; only the system UID may supply caller identity. */
interface IAppHostDomainVerification {
    List<String> queryValidVerificationPackageNames(int uid);
    DomainVerificationInfo getDomainVerificationInfo(int uid, String packageName);
    int setDomainVerificationStatus(int uid, String id, in DomainSet domains, int state);
}
