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
import com.android.server.pm.parsing.PackageCacher;
import com.android.server.pm.parsing.pkg.AndroidPackageUtils;
import com.android.server.pm.pkg.AndroidPackage;
import com.android.server.pm.permission.PermissionManagerServiceInternal;
import com.android.server.pm.verify.domain.DomainVerificationManagerInternal;

/** Original package-policy owners, available independently of late services. */
public final class PackageBootstrapBridge extends IPackageBootstrapBridge.Stub {
    private final DomainVerificationManagerInternal domains;

    public PackageBootstrapBridge() { this(null); }
    public PackageBootstrapBridge(DomainVerificationManagerInternal domains) {
        this.domains = domains;
    }

    private static final long ENFORCE_NATIVE_SHARED_LIBRARY_DEPENDENCIES = 142191088L;
    private static final long SELINUX_LATEST_CHANGES = 143539591L;
    private static final long SELINUX_R_CHANGES = 168782947L;

    /** Early M4 policy handoff; original PMS remains until the C facade passes its gates (#798). */
    public static com.android.server.pm.PackageManagerService startPackageManager(Context context,
            com.android.server.pm.Installer installer,
            com.android.server.pm.verify.domain.DomainVerificationService domains,
            boolean factoryTest) {
        try {
            attach(domains);
        } catch (RemoteException failure) {
            throw new IllegalStateException("package bootstrap attach failed", failure);
        }
        return com.android.server.pm.PackageManagerService.main(context, installer, domains, factoryTest);
    }

    /** Called by the C facade before native scanning, after PlatformCompat starts. */
    public static void attach(DomainVerificationManagerInternal domains) throws RemoteException {
        if (domains == null) throw new IllegalArgumentException("missing domain owner");
        IBinder host = ServiceManager.checkService("aim.service_host");
        if (host == null) throw new IllegalStateException("native service host is unavailable");
        IServiceHost.Stub.asInterface(host).attachPackageBootstrapBridge(new PackageBootstrapBridge(domains));
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
    public int getSeInfoTargetSdkVersion(byte[] packageCache) {
        enforceSystemUid();
        if (packageCache == null) throw new IllegalArgumentException("missing parsed package");
        AndroidPackage pkg = (AndroidPackage) PackageCacher.fromCacheEntryStatic(packageCache);
        android.content.pm.ApplicationInfo appInfo = AndroidPackageUtils.generateAppInfoWithoutState(pkg);
        PlatformCompat compat = (PlatformCompat) ServiceManager.getService(
                Context.PLATFORM_COMPAT_SERVICE);
        if (compat == null) throw new IllegalStateException("platform_compat is unavailable");
        if (compat.isChangeEnabledInternal(SELINUX_LATEST_CHANGES, appInfo)) {
            return Math.max(android.os.Build.VERSION_CODES.CUR_DEVELOPMENT, pkg.getTargetSdkVersion());
        }
        if (compat.isChangeEnabledInternal(SELINUX_R_CHANGES, appInfo)) {
            return Math.max(android.os.Build.VERSION_CODES.R, pkg.getTargetSdkVersion());
        }
        return pkg.getTargetSdkVersion();
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

    @Override
    public byte[] getLegacyPermissionState(int appId, int[] userIds) {
        enforceSystemUid();
        PackageLegacyPermissions.validate(appId, userIds);
        PermissionManagerServiceInternal permissions = LocalServices.getService(
                PermissionManagerServiceInternal.class);
        if (permissions == null) throw new IllegalStateException("permission owner is unavailable");
        return PackageLegacyPermissions.capture(appId, userIds, permissions.getLegacyPermissionState(appId));
    }

    @Override
    public byte[] generateNewDomainId() {
        enforceSystemUid();
        return PackageDomainIds.generate(domains);
    }

    private static void enforceSystemUid() {
        if (Binder.getCallingUid() != Process.SYSTEM_UID) {
            throw new SecurityException("the package bridge serves the system uid only");
        }
    }
}
