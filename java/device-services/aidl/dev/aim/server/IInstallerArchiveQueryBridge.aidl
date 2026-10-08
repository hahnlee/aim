package dev.aim.server;
import android.graphics.Bitmap;
import android.content.pm.PackageInstaller;
/** Original LauncherApps/Resources/AppOps/graphics leaves; no PMS owner. */
interface IInstallerArchiveQueryBridge {
    byte[] getLauncherActivities(String packageName, int userId);
    int getArchiveOverlayMode(int uid, String packageName);
    int getArchiveOptOutMode(int uid, String packageName);
    byte[] includeArchiveOverlay(in Bitmap icon);
    byte[] getUnarchiveDraftParams(String packageName, int userId, int launcherUid, String launcherPackage);
}
