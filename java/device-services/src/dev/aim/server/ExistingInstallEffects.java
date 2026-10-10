package dev.aim.server;

import android.content.Context;
import android.content.Intent;
import android.content.IntentSender;
import android.os.Parcel;
import android.os.ServiceManager;
import com.android.server.LocalServices;
import com.android.server.pm.Installer;
import com.android.server.pm.UserManagerInternal;
import com.android.server.pm.permission.PermissionManagerServiceInternal;

/** Original permission, installd, storage and backup owners after native admission. */
public final class ExistingInstallEffects {
    private ExistingInstallEffects() {}

    private static Context context() {
        var thread = android.app.ActivityThread.currentActivityThread();
        if (thread == null) throw new IllegalStateException("existing install context unavailable");
        return thread.getSystemContext();
    }

    public static byte[] userPolicy(int user) {
        if (user < 0) throw new IllegalArgumentException("Invalid userId " + user);
        var users = LocalServices.getService(UserManagerInternal.class);
        if (users == null) throw new IllegalStateException("existing install user owner unavailable");
        var policy = LocalServices.getService(android.app.admin.DevicePolicyManagerInternal.class);
        var parcel = Parcel.obtain();
        try {
            parcel.writeInt(user); parcel.writeBoolean(users.exists(user));
            parcel.writeBoolean(users.hasUserRestriction(android.os.UserManager.DISALLOW_INSTALL_APPS, user));
            parcel.writeBoolean(users.hasUserRestriction(android.os.UserManager.DISALLOW_DEBUGGING_FEATURES, user));
            parcel.writeBoolean(policy != null && policy.isUserOrganizationManaged(user));
            if (users != LocalServices.getService(UserManagerInternal.class)
                    || policy != LocalServices.getService(android.app.admin.DevicePolicyManagerInternal.class))
                throw new IllegalStateException("existing install policy owner changed");
            return parcel.marshall();
        } finally { parcel.recycle(); }
    }
    public static long[] installed(Installer installer, android.os.IBinder recordBinder, int user, int flags) throws Exception {
        if (installer == null) throw new IllegalStateException("existing install installd owner unavailable");
        var source = IExistingInstallRecord.Stub.asInterface(java.util.Objects.requireNonNull(recordBinder));
        byte[] record;
        try {
            int length = source.getLength();
            if (length < 0) throw new IllegalStateException("existing install record unavailable");
            record = new byte[length];
            for (int offset=0;offset<length;) {
                int count = Math.min(131072,length-offset);
                byte[] chunk = source.getChunk(offset,count);
                if (chunk == null || chunk.length != count) throw new IllegalStateException("existing install record truncated");
                System.arraycopy(chunk,0,record,offset,count); offset+=count;
            }
        } finally { source.close(); }
        var parcel = Parcel.obtain();
        try {
            parcel.unmarshall(record, 0, record.length); parcel.setDataPosition(0);
            if (parcel.readInt() != 1) throw new IllegalArgumentException("existing install record version");
            long version = parcel.readLong();
            var settingParcel = bytes(parcel.createByteArray());
            var codeParcel = bytes(parcel.createByteArray());
            int count = parcel.readInt();
            if (count < 0 || count > parcel.dataAvail() / 4) throw new IllegalArgumentException("existing install user count");
            var userStates = new java.util.ArrayList<PackageUserStateData>();
            for (int index = 0; index < count; index++) {
                var userParcel = bytes(parcel.createByteArray());
                try { userStates.add(PackageUserStateData.read(userParcel)); userParcel.enforceNoDataAvail(); }
                finally { userParcel.recycle(); }
            }
            String seInfo = java.util.Objects.requireNonNull(parcel.readString());
            boolean clearArchive = parcel.readBoolean();
            parcel.enforceNoDataAvail();
            PackageSettingData setting;
            PackageCode code;
            PackageUserStateData state = userStates.stream().filter(value -> value.getUserId() == user).findFirst().orElseThrow();
            try {
                setting = PackageSettingData.read(settingParcel); settingParcel.enforceNoDataAvail();
                code = PackageCode.CREATOR.createFromParcel(codeParcel); codeParcel.enforceNoDataAvail();
            } finally { settingParcel.recycle(); codeParcel.recycle(); }
            var packageSetting = com.android.server.pm.CapturedPackageSetting.withUsers(setting,
                    userStates, version, false, true);
            PackageObjects.restoreCollectedCode(packageSetting, code, version, false);
            var pkg = packageSetting.getPkg();
            if (clearArchive) {
                var icons = new java.io.File("/data/system_ce/" + user + "/package_archiver/" + setting.getPackageName());
                if (icons.exists() && !android.os.FileUtils.deleteContentsAndDir(icons))
                    throw new java.io.IOException("existing install archive icons could not be removed");
            }
            writeKernelMapping(packageSetting);
            var permission = LocalServices.getService(PermissionManagerServiceInternal.class);
            if (permission == null) throw new IllegalStateException("existing install permission owner unavailable");
            var permissionParams = new PermissionManagerServiceInternal.PackageInstalledParams.Builder();
            if ((flags & 0x00400000) != 0)
                permissionParams.setAllowlistedRestrictedPermissions(new java.util.ArrayList<>(pkg.getRequestedPermissions()));
            permission.onPackageInstalled(pkg, -1, permissionParams.build(), user);
            if (permission != LocalServices.getService(PermissionManagerServiceInternal.class))
                throw new IllegalStateException("existing install permission owner changed");
            var property = pkg.getProperties().get("android.internal.PROPERTY_NO_APP_DATA_STORAGE");
            if (setting.appId < 0 || property != null && property.getBoolean())
                return new long[] {0, state.ceDataInode, state.deDataInode};
            var users = LocalServices.getService(UserManagerInternal.class);
            var storage = LocalServices.getService(android.os.storage.StorageManagerInternal.class);
            if (users == null || storage == null) throw new IllegalStateException("existing install storage owners unavailable");
            int storageFlags;
            if (android.os.storage.StorageManager.isCeStorageUnlocked(user) && storage.isCeStoragePrepared(user)) storageFlags = 3;
            else if (users.isUserRunning(user)) storageFlags = 1;
            else return new long[] {0, state.ceDataInode, state.deDataInode};
            var args = new android.os.CreateAppDataArgs();
            args.uuid = setting.volumeUuid; args.packageName = setting.getPackageName();
            args.userId = user; args.flags = storageFlags | (packageSetting.getUsesSdkLibraries().length > 0 ? 8 : 0);
            args.appId = setting.appId; args.previousAppId = 0;
            args.seInfo = seInfo + com.android.server.pm.pkg.SELinuxUtil.getSeinfoUser(packageSetting.readUserState(user));
            args.targetSdkVersion = setting.targetSdkVersion;
            android.os.CreateAppDataResult result;
            synchronized (installer) {
                try {
                    var batch = new Installer.Batch();
                    var future = batch.createAppData(args); batch.execute(installer); result = future.join();
                }
                catch (Installer.InstallerException | java.util.concurrent.CompletionException failure) {
                    installer.destroyAppData(args.uuid, args.packageName, user, storageFlags, state.ceDataInode);
                    result = installer.createAppData(args);
                }
                if (result.exceptionCode != 0) throw new Installer.InstallerException(result.exceptionMessage);
                if ((storageFlags & 2) != 0 && setting.primaryCpuAbiRaw != null
                        && !dalvik.system.VMRuntime.is64BitAbi(setting.primaryCpuAbiRaw)
                        && new java.io.File(pkg.getNativeLibraryDir()).exists())
                    installer.linkNativeLibraryDirectory(setting.volumeUuid, setting.getPackageName(), pkg.getNativeLibraryDir(), user);
                if (users.isUserUnlockingOrUnlocked(user))
                    storage.prepareAppDataAfterInstall(setting.getPackageName(), user * 100000 + setting.appId);
            }
            if (users != LocalServices.getService(UserManagerInternal.class)
                    || storage != LocalServices.getService(android.os.storage.StorageManagerInternal.class))
                throw new IllegalStateException("existing install storage owner changed");
            return new long[] {storageFlags, result.ceDataInode, result.deDataInode};
        } finally { parcel.recycle(); }
    }
    private static final java.util.Map<String, int[]> kernelUsers = new java.util.HashMap<>();
    private static synchronized void writeKernelMapping(com.android.server.pm.PackageSetting setting) throws java.io.IOException {
        var root = new java.io.File("/config/sdcardfs");
        if (!root.exists()) return;
        String name = setting.getPackageName(); var directory = new java.io.File(root, name);
        if (!directory.isDirectory() && !directory.mkdir()) throw new java.io.IOException("kernel package mapping directory unavailable");
        writeKernelInt(new java.io.File(directory,"appid"),setting.getAppId());
        int[] excluded = setting.getNotInstalledUserIds(); int[] previous = kernelUsers.get(name);
        for (int id : excluded) if (previous == null || java.util.Arrays.stream(previous).noneMatch(value -> value == id))
            writeKernelInt(new java.io.File(directory,"excluded_userids"),id);
        if (previous != null) for (int id : previous) if (java.util.Arrays.stream(excluded).noneMatch(value -> value == id))
            writeKernelInt(new java.io.File(directory,"clear_userid"),id);
        kernelUsers.put(name,excluded.clone());
    }
    private static void writeKernelInt(java.io.File file,int value) throws java.io.IOException {
        try (var out = new java.io.FileOutputStream(file)) { out.write(Integer.toString(value).getBytes(java.nio.charset.StandardCharsets.US_ASCII)); }
    }
    private static Parcel bytes(byte[] bytes) {
        java.util.Objects.requireNonNull(bytes);
        var parcel = Parcel.obtain(); parcel.unmarshall(bytes, 0, bytes.length); parcel.setDataPosition(0); return parcel;
    }
    public static boolean restore(String name, int user, int token) throws android.os.RemoteException {
        var backup = android.app.backup.IBackupManager.Stub.asInterface(ServiceManager.checkService("backup"));
        if (backup == null || !backup.isUserReadyForBackup(user)) return false;
        backup.restoreAtInstallForUser(user, name, token); return true;
    }
    public static String complete(String name, int user, IntentSender target, int status, boolean restorePermissions) throws Exception {
        if (restorePermissions && status == 1 && name != null) {
            var permission = LocalServices.getService(PermissionManagerServiceInternal.class);
            if (permission == null) throw new IllegalStateException("existing install permission restore owner unavailable");
            permission.restoreDelayedRuntimePermissions(name, user);
            if (permission != LocalServices.getService(PermissionManagerServiceInternal.class))
                throw new IllegalStateException("existing install permission restore owner changed");
        }
        if (target != null) {
            var options = android.app.BroadcastOptions.makeBasic();
            options.setPendingIntentBackgroundActivityLaunchAllowed(false);
            try { target.sendIntent(context(), 0,
                    new Intent().putExtra("android.content.pm.extra.STATUS", android.content.pm.PackageManager.installStatusToPublicStatus(status)),
                    null, options.toBundle(), null, null); }
            catch (IntentSender.SendIntentException cancelled) { return cancelled.toString(); }
        }
        return null;
    }
}
