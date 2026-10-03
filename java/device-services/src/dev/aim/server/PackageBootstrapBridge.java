package dev.aim.server;

import android.content.Context;
import android.os.Binder;
import android.os.IBinder;
import android.os.Process;
import android.os.RemoteException;
import android.os.ServiceManager;
import com.android.server.LocalServices;
import com.android.server.compat.PlatformCompat;
import com.android.server.pm.parsing.library.PackageBackwardCompatibility;
import com.android.server.pm.permission.PermissionManagerServiceInternal;

/** Original package-policy owners, available independently of late services. */
public final class PackageBootstrapBridge extends IPackageBootstrapBridge.Stub {
    private static final long ENFORCE_NATIVE_SHARED_LIBRARY_DEPENDENCIES = 142191088L;

    /** Called by the C facade before native scanning, after PlatformCompat starts. */
    public static void attach() throws RemoteException {
        IBinder host = ServiceManager.checkService("aim.service_host");
        if (host == null) throw new IllegalStateException("native service host is unavailable");
        IServiceHost.Stub.asInterface(host).attachPackageBootstrapBridge(new PackageBootstrapBridge());
    }

    @Override
    public boolean areNativeLibraryDependenciesEnforced(String packageName, int targetSdk) {
        enforceSystemUid();
        PlatformCompat compat = (PlatformCompat) ServiceManager.getService(
                Context.PLATFORM_COMPAT_SERVICE);
        if (compat == null) throw new IllegalStateException("platform_compat is unavailable");
        return compat.isChangeEnabledInternal(
                ENFORCE_NATIVE_SHARED_LIBRARY_DEPENDENCIES, packageName, targetSdk);
    }

    @Override
    public boolean isTestBaseOnBootclasspath() {
        enforceSystemUid();
        return PackageBackwardCompatibility.bootClassPathContainsATB();
    }

    @Override
    public int[] getPermissionGidsForUid(int uid) {
        enforceSystemUid();
        if (uid < 0) throw new IllegalArgumentException("negative permission UID");
        PermissionManagerServiceInternal permissions = LocalServices.getService(
                PermissionManagerServiceInternal.class);
        if (permissions == null) throw new IllegalStateException("permission owner is unavailable");
        return permissions.getGidsForUid(uid);
    }

    private static void enforceSystemUid() {
        if (Binder.getCallingUid() != Process.SYSTEM_UID) {
            throw new SecurityException("the package bridge serves the system uid only");
        }
    }
}
