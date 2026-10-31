// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.pm.permission;

public interface LegacyPermissionManagerInternal {
    void resetRuntimePermissions();
    interface SyncAdapterPackagesProvider { String[] getPackages(String authority, int userId); }
    void setSyncAdapterPackagesProvider(SyncAdapterPackagesProvider provider);
    void setDialerAppPackagesProvider(PackagesProvider provider);
    void setSimCallManagerPackagesProvider(PackagesProvider provider);
    void setSmsAppPackagesProvider(PackagesProvider provider);
    void setUseOpenWifiAppPackagesProvider(PackagesProvider provider);
    void setVoiceInteractionPackagesProvider(PackagesProvider provider);
    void grantDefaultPermissionsToDefaultSimCallManager(String name, int user);
    void grantDefaultPermissionsToDefaultUseOpenWifiApp(String name, int user);
    int checkSoundTriggerRecordAudioPermissionForDataDelivery(int uid, String name, String tag, String reason);
    interface PackagesProvider {
        String[] getPackages(int userId);
    }

    void setLocationPackagesProvider(PackagesProvider provider);
    void setLocationExtraPackagesProvider(PackagesProvider provider);
    void grantDefaultPermissions(int userId);
    void scheduleReadDefaultPermissionExceptions();
}
