// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.pm;

import com.android.server.pm.pkg.PackageState;
import com.android.server.pm.pkg.SharedUserApi;

import java.util.Map;

public interface PackageManagerLocal {
    void reconcileSdkData(String volumeUuid, String packageName, java.util.List<String> subDirNames,
            int userId, int appId, int previousAppId, String seInfo, int flags) throws java.io.IOException;
    UnfilteredSnapshot withUnfilteredSnapshot();
    FilteredSnapshot withFilteredSnapshot();
    FilteredSnapshot withFilteredSnapshot(int callingUid, android.os.UserHandle user);
    void addOverrideSigningDetails(android.content.pm.SigningDetails oldDetails, android.content.pm.SigningDetails newDetails);
    void removeOverrideSigningDetails(android.content.pm.SigningDetails oldDetails);
    void clearOverrideSigningDetails();

    interface UnfilteredSnapshot extends AutoCloseable {
        FilteredSnapshot filtered(int callingUid, android.os.UserHandle user);
        Map<String, PackageState> getPackageStates();
        Map<String, PackageState> getDisabledSystemPackageStates();
        Map<String, SharedUserApi> getSharedUsers();
        @Override
        void close();
    }
    interface FilteredSnapshot extends AutoCloseable {
        PackageState getPackageState(String packageName);
        Map<String, PackageState> getPackageStates();
        @Override
        void close();
    }
}
