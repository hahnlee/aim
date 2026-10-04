// Compile-only pinned image API; checked by the device-services build node.
package com.android.server.pm.pkg;

import android.content.pm.SigningDetails;
import android.util.SparseArray;
import com.android.internal.pm.parsing.pkg.AndroidPackageInternal;
import com.android.server.pm.InstallSource;
import com.android.server.pm.PackageKeySetData;
import com.android.server.pm.permission.LegacyPermissionState;
import java.io.File;
import java.util.Set;
import java.util.UUID;

public interface PackageStateInternal extends PackageState {
    AndroidPackageInternal getPkg();
    PackageStateUnserialized getTransientState();
    UUID getDomainSetId();
    SigningDetails getSigningDetails();
    InstallSource getInstallSource();
    int getFlags();
    int getPrivateFlags();
    SparseArray<? extends PackageUserStateInternal> getUserStates();
    default PackageUserStateInternal getUserStateOrDefault(int user) {
        PackageUserStateInternal state = getUserStates().get(user);
        return state == null ? PackageUserStateInternal.DEFAULT : state;
    }
    LegacyPermissionState getLegacyPermissionState();
    String getRealName();
    boolean isLoading();
    String getPathString();
    float getLoadingProgress();
    long getLoadingCompletedTime();
    PackageKeySetData getKeySetData();
    String getPrimaryCpuAbiLegacy();
    String getSecondaryCpuAbiLegacy();
    String getAppMetadataFilePath();
    Set<File> getOldPaths();
    int getAppMetadataSource();
}
