package dev.aim.server;

import android.app.ActivityManager;
import android.app.AppOpsManager;
import android.app.BroadcastOptions;
import android.app.PendingIntent;
import android.app.admin.DevicePolicyManager;
import android.app.admin.DevicePolicyManagerInternal;
import android.content.Context;
import android.content.Intent;
import android.content.IntentSender;
import android.content.pm.ApplicationInfo;
import android.content.pm.ArchivedPackageInfo;
import android.content.pm.ArchivedPackageParcel;
import android.content.pm.IPackageDeleteObserver2;
import android.content.pm.LauncherActivityInfo;
import android.content.pm.LauncherApps;
import android.content.pm.PackageInstaller;
import android.content.pm.PackageManager;
import android.content.pm.PackageManagerInternal;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Rect;
import android.graphics.drawable.AdaptiveIconDrawable;
import android.graphics.drawable.BitmapDrawable;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.InsetDrawable;
import android.net.Uri;
import android.os.Binder;
import android.os.Environment;
import android.os.IInstalld;
import android.os.IBinder;
import android.os.ParcelableException;
import android.os.SELinux;
import android.os.ServiceManager;
import android.os.UserHandle;
import android.os.UserManager;
import android.os.PowerExemptionManager;
import com.android.server.LocalServices;
import com.android.server.pm.UserManagerInternal;
import com.android.server.pm.permission.PermissionManagerServiceInternal;
import com.android.server.wm.ActivityTaskManagerInternal;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Objects;

/** Original graphics/policy/AM leaves. No PackageManagerService instance. */
public final class InstallerRemovalBridge extends IInstallerRemovalBridge.Stub {
    private final Context context;
    public InstallerRemovalBridge(Context context) { this.context = Objects.requireNonNull(context); }
    private static void enforce() { enforceNativeOwner(); }
    private UserManagerInternal users() {
        var owner = LocalServices.getService(UserManagerInternal.class);
        if (owner == null) throw new IllegalStateException("UserManager owner unavailable");
        return owner;
    }
    private IInstalld installd() {
        IBinder binder = ServiceManager.checkService("installd");
        if (binder == null) throw new IllegalStateException("installd owner unavailable");
        return IInstalld.Stub.asInterface(binder);
    }
    @Override public boolean isSdkLibraryIndependenceEnabled() {
        enforce();
        return com.android.internal.hidden_from_bootclasspath.android.content.pm.Flags.sdkLibIndependence();
    }
    @Override public int checkPermission(String permission, int pid, int uid) {
        enforce(); return context.checkPermission(permission, pid, uid);
    }
    @Override public void checkPackage(int uid, String packageName) {
        enforce(); context.getSystemService(AppOpsManager.class).checkPackage(uid, packageName);
    }
    @Override public boolean canSilentlyInstall(String packageName, int uid) {
        enforce(); var owner = LocalServices.getService(DevicePolicyManagerInternal.class);
        return owner != null && owner.canSilentlyInstallPackage(packageName, uid);
    }
    @Override public boolean hasActiveAdmin(String packageName, int user) {
        enforce(); var owner = context.getSystemService(DevicePolicyManager.class);
        return owner != null && owner.packageHasActiveAdmins(packageName, user);
    }
    @Override public boolean isPinned(String packageName) {
        enforce(); var owner = LocalServices.getService(ActivityTaskManagerInternal.class);
        if (owner == null) throw new IllegalStateException("ActivityTaskManager owner unavailable");
        return owner.isBaseOfLockedTask(packageName);
    }
    @Override public boolean isUninstallRestricted(int user) {
        enforce(); return context.getSystemService(UserManager.class).hasUserRestriction(
            UserManager.DISALLOW_UNINSTALL_APPS, UserHandle.of(user));
    }
    @Override public int[] getUsers() { enforce(); return users().getUserIds(); }
    @Override public int[] getChildrenDeletedWithParent(int user) {
        enforce(); var owner = users(); var result = new ArrayList<Integer>();
        for (int child : owner.getProfileIds(user, true)) {
            if (child == user || owner.getProfileParentId(child) != user) continue;
            var properties = owner.getUserProperties(child);
            if (properties != null && properties.getDeleteAppWithParent()) result.add(child);
        }
        int[] ids = new int[result.size()];
        for (int i = 0; i < ids.length; i++) ids[i] = result.get(i);
        return ids;
    }
    @Override public boolean isArchiveOptedOut(String packageName, int uid) {
        enforce(); return context.getSystemService(AppOpsManager.class).checkOpNoThrow(
            AppOpsManager.OP_AUTO_REVOKE_PERMISSIONS_IF_UNUSED, uid, packageName) == AppOpsManager.MODE_IGNORED;
    }
    private String installerTitle(String installer, int user) throws PackageManager.NameNotFoundException {
        var scope = context.createPackageContextAsUser(installer, 0, UserHandle.of(user));
        var info = scope.getPackageManager().getApplicationInfo(installer, 0);
        return info.loadLabel(scope.getPackageManager()).toString();
    }
    private File storeIcon(String packageName, Drawable drawable, int user, int index, boolean adaptive) throws IOException {
        if (drawable == null) return null;
        if (adaptive) {
            float fraction = AdaptiveIconDrawable.getExtraInsetFraction();
            float inset = fraction / (1 + 2 * fraction);
            drawable = new AdaptiveIconDrawable(new ColorDrawable(Color.BLACK),
                new InsetDrawable(drawable, inset, inset, inset, inset));
        }
        int size = context.getSystemService(ActivityManager.class).getLauncherLargeIconSize();
        Bitmap bitmap;
        if (drawable instanceof BitmapDrawable && ((BitmapDrawable) drawable).getBitmap() != null
                && drawable.getIntrinsicWidth() < size && drawable.getIntrinsicHeight() < size) {
            bitmap = ((BitmapDrawable) drawable).getBitmap();
        } else {
            int width = drawable.getIntrinsicWidth(), height = drawable.getIntrinsicHeight();
            if (width <= 0) width = size;
            if (height <= 0) height = size;
            float scale = Math.min(1f, (float) size / Math.max(width, height));
            bitmap = Bitmap.createBitmap(Math.max(1, (int) (width * scale)), Math.max(1, (int) (height * scale)), Bitmap.Config.ARGB_8888);
            Rect saved = new Rect(drawable.getBounds());
            drawable.setBounds(0, 0, bitmap.getWidth(), bitmap.getHeight());
            drawable.draw(new Canvas(bitmap));
            drawable.setBounds(saved);
        }
        File directory = new File(new File(Environment.getDataSystemCeDirectory(user), "package_archiver"), packageName);
        if (!directory.isDirectory() && !directory.mkdirs()) throw new IOException("Cannot create archive icon directory");
        if (!SELinux.restorecon(directory)) throw new IOException("Cannot label archive icon directory");
        File path = new File(directory, index + ".png");
        try (FileOutputStream output = new FileOutputStream(path)) {
            if (!bitmap.compress(Bitmap.CompressFormat.PNG, 100, output)) throw new IOException("Cannot encode archive icon");
            output.flush();
        }
        return path;
    }
    @Override public InstallerArchiveMetadata collectArchive(String packageName, String installer, int user) {
        enforce();
        try {
            var launcher = context.getSystemService(LauncherApps.class);
            var activities = launcher.getActivityList(packageName, UserHandle.of(user));
            if (activities.isEmpty()) throw new PackageManager.NameNotFoundException("The app " + packageName + " does not have a main activity.");
            var result = new InstallerArchiveMetadata(); result.installerTitle = installerTitle(installer, user);
            result.archiveTime = System.currentTimeMillis(); result.activities = new InstallerArchiveActivity[activities.size()];
            for (int i = 0; i < activities.size(); i++) {
                var source = activities.get(i); var target = new InstallerArchiveActivity();
                target.title = source.getLabel().toString(); target.component = source.getComponentName().flattenToString();
                if (source.getActivityInfo().getIconResource() != 0) {
                    File path = storeIcon(packageName, source.getIcon(0), user, i * 2, false);
                    target.icon = path == null ? null : path.getAbsolutePath();
                }
                result.activities[i] = target;
            }
            return result;
        } catch (IOException | PackageManager.NameNotFoundException error) { throw new ParcelableException(error); }
    }
    @Override public InstallerArchiveMetadata collectArchived(ArchivedPackageParcel archived, String installer, int user) {
        enforce();
        try {
            var source = new ArchivedPackageInfo(Objects.requireNonNull(archived));
            var activities = source.getLauncherActivities(); var result = new InstallerArchiveMetadata();
            result.installerTitle = installerTitle(installer, user); result.archiveTime = System.currentTimeMillis();
            result.activities = new InstallerArchiveActivity[activities.size()];
            for (int i = 0; i < activities.size(); i++) {
                var activity = activities.get(i); var target = new InstallerArchiveActivity();
                target.title = activity.getLabel().toString(); target.component = activity.getComponentName().flattenToString();
                File icon = storeIcon(source.getPackageName(), activity.getIcon(), user, i * 2, true);
                File monochrome = storeIcon(source.getPackageName(), activity.getMonochromeIcon(), user, i * 2 + 1, true);
                target.icon = icon == null ? null : icon.getAbsolutePath();
                target.monochrome = monochrome == null ? null : monochrome.getAbsolutePath(); result.activities[i] = target;
            }
            return result;
        } catch (IOException | PackageManager.NameNotFoundException error) { throw new ParcelableException(error); }
    }
    @Override public void destroyAppData(String volume, String packageName, int user, long ceInode) throws android.os.RemoteException {
        enforce(); installd().destroyAppData(volume, packageName, user, IInstalld.FLAG_STORAGE_DE | IInstalld.FLAG_STORAGE_CE | IInstalld.FLAG_STORAGE_EXTERNAL, ceInode);
    }
    @Override public void clearArchiveCaches(String volume, String packageName, int user, long ceInode) throws android.os.RemoteException {
        enforce(); int storage = IInstalld.FLAG_STORAGE_DE | IInstalld.FLAG_STORAGE_CE | IInstalld.FLAG_STORAGE_EXTERNAL;
        installd().clearAppData(volume, packageName, user, storage | IInstalld.FLAG_CLEAR_CACHE_ONLY, ceInode);
        installd().clearAppData(volume, packageName, user, storage | IInstalld.FLAG_CLEAR_CODE_CACHE_ONLY, ceInode);
    }
    @Override public void destroyProfiles(String packageName) throws android.os.RemoteException { enforce(); installd().destroyAppProfiles(packageName); }
    @Override public void removeCode(String packageName, String path) throws android.os.RemoteException { enforce(); installd().rmPackageDir(packageName, path); }
    @Override public IBinder preparePermissions(String packageName, int appId, int user) {
        enforce(); var packages = LocalServices.getService(PackageManagerInternal.class);
        var permissions = LocalServices.getService(PermissionManagerServiceInternal.class);
        if (packages == null || permissions == null) throw new IllegalStateException("Package permission owners unavailable");
        var state = packages.getPackageStateInternal(packageName);
        if (state == null) throw new IllegalStateException("Native removed-package scope unavailable");
        var code = state.getAndroidPackage();
        var siblings = new ArrayList<com.android.server.pm.pkg.AndroidPackage>();
        var shared = packages.getSharedUserPackages(appId);
        if (shared != null) for (var sibling : shared) {
            if (!packageName.equals(sibling.getPackageName()) && sibling.getAndroidPackage() != null) siblings.add(sibling.getAndroidPackage());
        }
        return new IInstallerPermissionRemoval.Stub() {
            private boolean consumed;
            @Override public synchronized void apply(boolean codeRemoved, boolean clearGrants) {
                enforce(); if (consumed) throw new IllegalStateException("Removal permission scope already consumed");
                consumed = true;
                if (codeRemoved && code != null) permissions.onPackageRemoved(code);
                if (clearGrants) permissions.onPackageUninstalled(packageName, appId, state, code, siblings, user);
            }
        };
    }
    private void send(IntentSender receiver, Intent intent) {
        if (receiver == null) return;
        var options = BroadcastOptions.makeBasic(); options.setPendingIntentBackgroundActivityLaunchAllowed(false);
        try { receiver.sendIntent(context, 0, intent, null, options.toBundle(), null, null); }
        catch (IntentSender.SendIntentException error) { throw new ParcelableException(error); }
    }
    @Override public void sendUninstallStatus(IntentSender receiver, String packageName, int status, String message) {
        enforce(); Intent result = new Intent();
        result.putExtra(PackageInstaller.EXTRA_PACKAGE_NAME, packageName);
        result.putExtra(PackageInstaller.EXTRA_STATUS, PackageManager.deleteStatusToPublicStatus(status));
        result.putExtra(PackageInstaller.EXTRA_STATUS_MESSAGE, PackageManager.deleteStatusToString(status, message));
        result.putExtra(PackageInstaller.EXTRA_LEGACY_STATUS, status); send(receiver, result);
    }
    @Override public void sendArchivedInstallStatus(IntentSender receiver, int id, String packageName, int status, String message) {
        enforce(); Intent result = new Intent(); result.putExtra(PackageInstaller.EXTRA_SESSION_ID, id);
        result.putExtra(PackageInstaller.EXTRA_PACKAGE_NAME, packageName);
        result.putExtra(PackageInstaller.EXTRA_STATUS, PackageManager.installStatusToPublicStatus(status));
        result.putExtra(PackageInstaller.EXTRA_STATUS_MESSAGE, PackageManager.installStatusToString(status, message));
        result.putExtra(PackageInstaller.EXTRA_LEGACY_STATUS, status); send(receiver, result);
    }
    @Override public void sendUninstallUserAction(IntentSender receiver, String packageName, int flags, IPackageDeleteObserver2 observer) {
        enforce(); Intent action = new Intent(Intent.ACTION_UNINSTALL_PACKAGE);
        action.setData(Uri.fromParts("package", packageName, null));
        action.putExtra(PackageInstaller.EXTRA_CALLBACK, new PackageManager.UninstallCompleteCallback(observer.asBinder()));
        if ((flags & PackageManager.DELETE_ARCHIVE) != 0) action.putExtra(PackageInstaller.EXTRA_DELETE_FLAGS, flags);
        Intent result = new Intent(); result.putExtra(PackageInstaller.EXTRA_PACKAGE_NAME, packageName);
        result.putExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_PENDING_USER_ACTION);
        result.putExtra(Intent.EXTRA_INTENT, action); send(receiver, result);
    }
    @Override public void forwardUninstallUserAction(IntentSender receiver, String packageName, Intent action) {
        enforce(); Intent result = new Intent(); result.putExtra(PackageInstaller.EXTRA_PACKAGE_NAME, packageName);
        result.putExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_PENDING_USER_ACTION);
        result.putExtra(Intent.EXTRA_INTENT, action); send(receiver, result);
    }
    @Override public void sendDeleteUserAction(String packageName, int flags, IPackageDeleteObserver2 observer) throws android.os.RemoteException {
        enforce(); Intent intent = new Intent(Intent.ACTION_UNINSTALL_PACKAGE);
        intent.setData(Uri.fromParts("package", packageName, null));
        intent.putExtra(PackageInstaller.EXTRA_CALLBACK, new PackageManager.UninstallCompleteCallback(observer.asBinder()));
        if ((flags & PackageManager.DELETE_ARCHIVE) != 0) intent.putExtra(PackageInstaller.EXTRA_DELETE_FLAGS, flags);
        observer.onUserActionRequired(intent);
    }
    @Override public void sendUnarchiveConfirmation(IntentSender receiver, String packageName, int user) {
        enforce(); Intent action = new Intent("com.android.intent.action.UNARCHIVE_DIALOG");
        action.putExtra("android.content.pm.extra.UNARCHIVE_INTENT_SENDER", receiver);
        action.putExtra(PackageInstaller.EXTRA_PACKAGE_NAME, packageName);
        Intent result = new Intent(); result.putExtra(PackageInstaller.EXTRA_PACKAGE_NAME, packageName);
        result.putExtra("android.content.pm.extra.UNARCHIVE_STATUS", PackageInstaller.STATUS_PENDING_USER_ACTION);
        result.putExtra(Intent.EXTRA_INTENT, action); result.putExtra(Intent.EXTRA_USER, UserHandle.of(user)); send(receiver, result);
    }
    @Override public void sendUnarchiveStatus(IntentSender receiver, String packageName, String installer, String title, int user, int status, long requiredBytes, PendingIntent action) {
        enforce(); Intent result = new Intent(); result.putExtra(PackageInstaller.EXTRA_PACKAGE_NAME, packageName);
        result.putExtra("android.content.pm.extra.UNARCHIVE_STATUS", status);
        if (status != 0) {
            Intent dialog = new Intent("com.android.intent.action.UNARCHIVE_ERROR_DIALOG");
            dialog.putExtra("android.content.pm.extra.UNARCHIVE_STATUS", status);
            dialog.putExtra(Intent.EXTRA_USER, UserHandle.of(user));
            dialog.putExtra("com.android.content.pm.extra.UNARCHIVE_INSTALLER_PACKAGE_NAME", installer);
            dialog.putExtra("com.android.content.pm.extra.UNARCHIVE_INSTALLER_TITLE", title);
            if (requiredBytes > 0) dialog.putExtra("com.android.content.pm.extra.UNARCHIVE_EXTRA_REQUIRED_BYTES", requiredBytes);
            if (action != null) dialog.putExtra(Intent.EXTRA_INTENT, action);
            result.putExtra(Intent.EXTRA_INTENT, dialog); result.putExtra(Intent.EXTRA_USER, UserHandle.of(user));
        }
        send(receiver, result);
    }
    @Override public void broadcastUnarchive(String packageName, String installer, int user, int id, boolean allUsers) {
        enforce(); Intent intent = new Intent(Intent.ACTION_UNARCHIVE_PACKAGE).setPackage(installer);
        intent.addFlags(Intent.FLAG_RECEIVER_FOREGROUND);
        intent.putExtra(PackageInstaller.EXTRA_UNARCHIVE_ID, id);
        intent.putExtra(PackageInstaller.EXTRA_UNARCHIVE_PACKAGE_NAME, packageName);
        intent.putExtra(PackageInstaller.EXTRA_UNARCHIVE_ALL_USERS, allUsers);
        var options = BroadcastOptions.makeBasic();
        options.setTemporaryAppAllowlist(120000, PowerExemptionManager.TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED, PowerExemptionManager.REASON_PACKAGE_UNARCHIVE, "");
        context.sendOrderedBroadcastAsUser(intent, UserHandle.of(user), null, -1, options.toBundle(), null, null, 0, null, null);
    }
    @Override public void sendRemovedBroadcast(String packageName, int appId, int user, boolean dataRemoved, boolean fullyRemoved, boolean uidRemoved, long version) {
        enforce(); int uid = UserHandle.getUid(user, appId);
        Intent removed = new Intent(Intent.ACTION_PACKAGE_REMOVED, Uri.fromParts("package", packageName, null));
        removed.putExtra(Intent.EXTRA_UID, uid); removed.putExtra(Intent.EXTRA_DATA_REMOVED, dataRemoved);
        removed.putExtra(Intent.EXTRA_DONT_KILL_APP, false); removed.putExtra(Intent.EXTRA_USER_INITIATED, true);
        context.sendBroadcastAsUser(removed, UserHandle.of(user));
        if (fullyRemoved && dataRemoved) {
            Intent full = new Intent(Intent.ACTION_PACKAGE_FULLY_REMOVED, removed.getData());
            full.putExtra(Intent.EXTRA_UID, uid); full.putExtra(Intent.EXTRA_DATA_REMOVED, true);
            context.sendBroadcastAsUser(full, UserHandle.of(user));
        }
        if (uidRemoved) { Intent gone = new Intent(Intent.ACTION_UID_REMOVED); gone.putExtra(Intent.EXTRA_UID, uid); context.sendBroadcastAsUser(gone, UserHandle.of(user)); }
    }
    private static void enforceNativeOwner() {
        if (android.os.Binder.getCallingUid() != android.os.Process.SYSTEM_UID)
            throw new SecurityException("native installer bridge requires system UID");
    }
}
