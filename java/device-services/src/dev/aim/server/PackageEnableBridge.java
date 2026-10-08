package dev.aim.server;
import android.content.Context;
import com.android.internal.hidden_from_bootclasspath.android.content.pm.Flags;
import android.content.pm.PackageManagerInternal;
import android.os.IInstalld;
import android.os.ServiceManager;
import com.android.server.LocalServices;
public final class PackageEnableBridge extends IPackageEnableBridge.Stub {
    public PackageEnableBridge(Context context) { java.util.Objects.requireNonNull(context); }
    @Override public boolean quarantineEnabled() { Bridge.enforceSystemUid(); return Flags.quarantinedEnabled(); }
    @Override public void clearCodeCache(String packageName, int[] users) throws android.os.RemoteException {
        Bridge.enforceSystemUid();
        var packages = LocalServices.getService(PackageManagerInternal.class);
        if (packages == null) throw new IllegalStateException("Native package scope unavailable");
        var state = packages.getPackageStateInternal(packageName);
        if (state == null) throw new IllegalStateException("Compressed package not published");
        var binder = ServiceManager.checkService("installd");
        if (binder == null) throw new IllegalStateException("installd owner unavailable");
        var installd = IInstalld.Stub.asInterface(binder);
        int flags = IInstalld.FLAG_STORAGE_DE | IInstalld.FLAG_STORAGE_CE | IInstalld.FLAG_STORAGE_EXTERNAL | IInstalld.FLAG_CLEAR_CODE_CACHE_ONLY;
        for (int user : users) installd.clearAppData(state.getVolumeUuid(), packageName, user, flags, state.getUserStateOrDefault(user).getCeDataInode());
    }
}
