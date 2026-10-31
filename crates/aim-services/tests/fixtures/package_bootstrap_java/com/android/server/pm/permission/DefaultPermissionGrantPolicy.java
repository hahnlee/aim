package com.android.server.pm.permission;
import android.content.pm.PackageManagerInternal;
import com.android.server.LocalServices;

/** Android boundary: retain the original constructor's local owner and exercise its four reads. */
final class DefaultPermissionGrantPolicy {
    private final PackageManagerInternal packages;
    DefaultPermissionGrantPolicy(android.content.Context context) {
        packages = java.util.Objects.requireNonNull(LocalServices.getService(PackageManagerInternal.class));
    }
    private LegacyPermissionManagerInternal.SyncAdapterPackagesProvider sync;
    public void setSyncAdapterPackagesProvider(LegacyPermissionManagerInternal.SyncAdapterPackagesProvider provider) { sync = provider; }
    public void setLocationPackagesProvider(LegacyPermissionManagerInternal.PackagesProvider provider) { throw new AssertionError(); }
    public void setLocationExtraPackagesProvider(LegacyPermissionManagerInternal.PackagesProvider provider) { throw new AssertionError(); }
    public void grantDefaultPermissions(int user) {
        if (sync != null && !java.util.Arrays.equals(sync.getPackages("calendar", user), new String[]{"sync.adapter"})) throw new AssertionError();
        if (LocalServices.getService(PackageManagerInternal.class) != packages) throw new AssertionError("owner replaced");
        if (!java.util.Arrays.equals(packages.getKnownPackageNames(1, user), new String[] {"known"})
                || !packages.getDisabledSystemPackageName("updated").equals("disabled")
                || !packages.isPlatformSigned("android")) throw new AssertionError("query lost");
        var component = new android.content.pm.ComponentInfo();
        component.packageName = "resolver"; component.name = "Resolver";
        if (!packages.isResolveActivityComponent(component)) throw new AssertionError("resolver lost");
    }
}
