package dev.aim.server;
import android.content.IntentFilter;

/** Native Settings, domain and package-user lifecycle operations. */
interface IPackageUserOperations {
    byte[] createUserState(int userId, in String[] installablePackages,
            in String[] disallowedPackages, long currentTimeMillis, boolean stopSystemPackages,
            in byte[] userRecord);
    void finishUserCreation(int userId);
    void cleanUpUserSettings(int userId);
    void finishUserRemoval(int userId);
    void readPermissionStateForUser(int userId);
    void clearDomainUser(int userId);
    boolean hasSystemFeature(String name, int version);
    boolean isDeviceUpgrading();
    void addCrossProfileIntentFilter(in IntentFilter filter, String ownerPackage,
            int sourceUserId, int targetUserId, int flags);
}
