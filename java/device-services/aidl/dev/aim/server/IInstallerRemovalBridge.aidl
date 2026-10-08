package dev.aim.server;
import android.content.IntentSender;
import android.content.Intent;
import android.app.PendingIntent;
import android.content.pm.IPackageDeleteObserver2;
import android.content.pm.ArchivedPackageParcel;
import android.os.Bundle;
import dev.aim.server.InstallerArchiveMetadata;
/** Original policy, graphic and AM owners; native PackageManager owns changes. */
interface IInstallerRemovalBridge {
    int checkPermission(String permission, int pid, int uid);
    void checkPackage(int uid, String packageName);
    boolean canSilentlyInstall(String packageName, int uid);
    boolean hasActiveAdmin(String packageName, int user);
    boolean isPinned(String packageName);
    boolean isUninstallRestricted(int user);
    int[] getUsers();
    int[] getChildrenDeletedWithParent(int user);
    boolean isArchiveOptedOut(String packageName, int uid);
    InstallerArchiveMetadata collectArchive(String packageName, String installer, int user);
    InstallerArchiveMetadata collectArchived(in ArchivedPackageParcel archived, String installer, int user);
    void destroyAppData(String volume, String packageName, int user, long ceInode);
    void clearArchiveCaches(String volume, String packageName, int user, long ceInode);
    void destroyProfiles(String packageName);
    void removeCode(String packageName, String path);
    IBinder preparePermissions(String packageName, int appId, int user);
    void sendUninstallStatus(in IntentSender receiver, String packageName, int status, String message);
    void sendArchivedInstallStatus(in IntentSender receiver, int sessionId, String packageName, int status, String message);
    void sendUninstallUserAction(in IntentSender receiver, String packageName, int flags, IPackageDeleteObserver2 observer);
    void forwardUninstallUserAction(in IntentSender receiver, String packageName, in Intent action);
    void sendDeleteUserAction(String packageName, int flags, IPackageDeleteObserver2 observer);
    void sendUnarchiveConfirmation(in IntentSender receiver, String packageName, int user);
    void sendUnarchiveStatus(in IntentSender receiver, String packageName, String installer, String installerTitle,
        int user, int status, long requiredBytes, in PendingIntent userAction);
    void broadcastUnarchive(String packageName, String installer, int user, int unarchiveId, boolean allUsers);
    void sendRemovedBroadcast(String packageName, int appId, int user, boolean dataRemoved, boolean fullyRemoved, boolean uidRemoved, long version);
    boolean isSdkLibraryIndependenceEnabled();
}
