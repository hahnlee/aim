package dev.aim.server;
import android.content.Context;
import android.os.Environment;
import android.os.IInstalld;
import android.os.ServiceManager;
import android.os.storage.StorageManager;
import android.os.storage.StorageManagerInternal;
import android.os.storage.VolumeInfo;
import com.android.server.LocalServices;
import com.android.server.pm.UserManagerInternal;
import java.io.File;
import java.util.ArrayList;
import java.util.Objects;

/** Original storage/installd leaves; package admission and publication are native. */
public final class PackageRelocationBridge extends IPackageRelocationBridge.Stub {
    private final Context context;
    public PackageRelocationBridge(Context context) {
        this.context = Objects.requireNonNull(context);
    }
    private static void enforce() { if (android.os.Binder.getCallingUid() != 1000) throw new SecurityException("Native relocation owner required"); }
    private StorageManager storage() {
        return Objects.requireNonNull(context.getSystemService(StorageManager.class), "StorageManager owner unavailable");
    }
    private IInstalld installd() {
        var binder = ServiceManager.checkService("installd");
        if (binder == null) throw new IllegalStateException("installd owner unavailable");
        return IInstalld.Stub.asInterface(binder);
    }
    private UserManagerInternal users() {
        var owner = LocalServices.getService(UserManagerInternal.class);
        if (owner == null) throw new IllegalStateException("UserManager owner unavailable");
        return owner;
    }
    @Override public boolean allowThirdPartyInternal() {
        enforce(); return context.getResources().getBoolean(com.android.internal.R.bool.config_allow3rdPartyAppOnInternal);
    }
    @Override public PackageRelocationVolume resolveVolume(String uuid) {
        enforce(); var result = new PackageRelocationVolume();
        if (Objects.equals(StorageManager.UUID_PRIVATE_INTERNAL, uuid)) {
            result.complete = true; result.measurePath = Environment.getDataAppDirectory(uuid).getAbsolutePath();
        } else if (Objects.equals(StorageManager.UUID_PRIMARY_PHYSICAL, uuid)) {
            var volume = storage().getPrimaryPhysicalVolume();
            if (volume == null) throw new IllegalStateException("Primary physical volume unavailable");
            result.complete = false; result.measurePath = volume.getPath().getAbsolutePath();
        } else {
            var volume = storage().findVolumeByUuid(uuid);
            if (volume == null || volume.getType() != VolumeInfo.TYPE_PRIVATE || !volume.isMountedWritable())
                throw new IllegalStateException("Move location not mounted private volume");
            result.complete = true; result.measurePath = Environment.getDataAppDirectory(uuid).getAbsolutePath();
        }
        return result;
    }
    @Override public boolean isFileEncrypted() { enforce(); return StorageManager.isFileEncrypted(); }
    @Override public boolean isUserUnlocked(int user) { enforce(); return StorageManager.isCeStorageUnlocked(user); }
    @Override public int[] getUsers() { enforce(); return users().getUserIds(); }
    @Override public String applicationLabel(String packageName, int user) {
        enforce();
        try {
            var pm = context.getPackageManager();
            return String.valueOf(pm.getApplicationLabel(pm.getApplicationInfoAsUser(packageName, 0, user)));
        } catch (android.content.pm.PackageManager.NameNotFoundException error) {
            throw new android.os.ParcelableException(error);
        }
    }
    @Override public long[] measurePackage(String volume, String packageName, int user, int appId, long ceInode, String codePath) throws android.os.RemoteException {
        enforce(); return installd().getAppSize(volume, new String[]{packageName}, user, 0, appId, new long[]{ceInode}, new String[]{codePath});
    }
    @Override public long usableBytes(String path) { enforce(); return new File(path).getUsableSpace(); }
    @Override public long bytesUntilLow(String path) { enforce(); return storage().getStorageBytesUntilLow(new File(path)); }
    @Override public void prepareUsers(String from, String to, int[] ids) {
        enforce(); var owner = LocalServices.getService(StorageManagerInternal.class);
        if (owner == null) throw new IllegalStateException("StorageManager owner unavailable");
        var users = users(); var records = new ArrayList<android.content.pm.UserInfo>();
        for (int id : ids) {
            var info = users.getUserInfo(id);
            if (info == null) throw new IllegalArgumentException("Unknown move user " + id);
            records.add(info);
        }
        owner.prepareUserStorageForMove(from, to, records);
    }
    @Override public void moveCompleteApp(String from, String to, String packageName, int appId, String seinfo, int targetSdk, String path) throws android.os.RemoteException {
        enforce(); installd().moveCompleteApp(from, to, packageName, appId, seinfo, targetSdk, path);
    }
}
