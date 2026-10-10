package com.android.server.pm;

import android.util.Slog;
import java.util.Objects;

/** Original installd batch used by native Settings.createNewUserLI. */
public final class NativeUserAppData {
    public static boolean stopSystemPackagesByDefault(android.content.Context context) {
        return Objects.requireNonNull(context).getResources().getBoolean(
                com.android.internal.R.bool.config_stopSystemPackagesByDefault);
    }
    public static void create(Installer installer, int userId, String[] volumeUuids,
            String[] packageNames, int[] storageFlags, int[] appIds, String[] seInfos,
            int[] targetSdkVersions) {
        Objects.requireNonNull(installer);
        Objects.requireNonNull(packageNames);
        int count = packageNames.length;
        if (userId < 0 || volumeUuids.length != count || storageFlags.length != count
                || appIds.length != count || seInfos.length != count
                || targetSdkVersions.length != count) {
            throw new IllegalArgumentException("new-user installd batch inventory differs");
        }
        Installer.Batch batch = new Installer.Batch();
        for (int i = 0; i < count; i++) {
            Objects.requireNonNull(packageNames[i]);
            if (appIds[i] < 0 || (storageFlags[i] != 1 && storageFlags[i] != 9)) {
                throw new IllegalArgumentException("new-user installd request is not DE storage");
            }
            // Native flags already include FLAG_STORAGE_SDK when the code uses
            // SDK libraries. buildCreateAppDataArgs retains the original defaults.
            batch.createAppData(Installer.buildCreateAppDataArgs(volumeUuids[i], packageNames[i],
                    userId, storageFlags[i], appIds[i], seInfos[i], targetSdkVersions[i], false));
        }
        try {
            batch.execute(installer);
        } catch (Installer.InstallerException failure) {
            // Settings logs this checked storage failure and continues with the
            // default preferred apps and scheduled restrictions/list publication.
            Slog.w("PackageManager", "Failed to prepare app data", failure);
        }
    }
    private NativeUserAppData() {}
}
