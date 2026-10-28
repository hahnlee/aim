package dev.aim.server;

import android.content.pm.verify.domain.DomainSet;
import android.content.pm.verify.domain.DomainVerificationInfo;
import android.content.UriRelativeFilterGroupParcel;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;

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
    int runShellCommand(boolean packageShell, in String[] arguments,
            in ParcelFileDescriptor input, in ParcelFileDescriptor output, in ParcelFileDescriptor error);
    byte[] readShellState(int kind, String packageName, in List<String> domains, int user, boolean hasUser);
    void setShellUserSelection(String packageName, int user, boolean enabled, in List<String> domains);
    void setShellLinkHandling(String packageName, int user, boolean allowed);
    void setShellStatus(String packageName, int state, in List<String> domains);
    void resetShellState(in List<String> packageNames, int user, boolean userOnly);
}
