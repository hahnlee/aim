package dev.aim.server;
import java.lang.reflect.Proxy;
import java.util.Arrays;
import java.util.List;
import com.android.server.pm.permission.PermissionManagerServiceInternal.PackageInstalledParams;

final class PackageLocalOwnersTest {
    @SuppressWarnings("unchecked")
    private static <T> T owner(Class<T> type, java.lang.reflect.InvocationHandler calls) {
        return (T) Proxy.newProxyInstance(type.getClassLoader(), new Class<?>[] {type}, calls);
    }
    static void run() {
        var calls = new java.util.ArrayList<String>();
        boolean[] dead = {false};
        var backend = owner(ISystemProcess.class, (proxy, method, args) -> {
            if (dead[0]) throw new android.os.RemoteException("owner died");
            if (method.getName().equals("isPackageCeStoragePrepared")) return (int) args[0] == 0;
            if (!method.getName().equals("existingPackagePermissionsInstalled")
                    || !args[0].equals("p") || (int) args[2] != 0) throw new AssertionError(method);
            calls.add(Arrays.toString((String[]) args[1])); return null;
        });
        var permissions = new PackageLocalOwners.Permissions(backend);
        var pkg = owner(com.android.server.pm.pkg.AndroidPackage.class, (proxy, method, args) -> {
            if (!method.getName().equals("getPackageName")) throw new AssertionError(method);
            return "p";
        });
        var builder = new PackageInstalledParams.Builder();
        permissions.onPackageInstalled(pkg, -1, builder.build(), 0);
        builder.setAllowlistedRestrictedPermissions(List.of("restricted"));
        permissions.onPackageInstalled(pkg, -1, builder.build(), 0);
        if (!calls.equals(List.of("[]", "[restricted]"))) throw new AssertionError(calls);
        try { permissions.onPackageInstalled(pkg, 12, builder.build(), 0); throw new AssertionError(); }
        catch (IllegalArgumentException expected) {}
        var storage = new PackageLocalOwners.Storage(backend);
        if (!storage.isCeStoragePrepared(0) || storage.isCeStoragePrepared(10)) throw new AssertionError();
        var users = new PackageLocalOwners.Users(owner(android.os.IUserManager.class, (proxy, method, args) -> {
            if (dead[0]) throw new android.os.RemoteException("owner died");
            switch (method.getName()) {
                case "getUserInfo": return null;
                case "isUserRunning": case "isUserUnlockingOrUnlocked": return (int) args[0] == 0;
                case "hasUserRestriction": return args[0].equals("no_install_apps") && (int) args[1] == 0;
                default: throw new AssertionError(method);
            }
        }));
        if (users.exists(10) || !users.isUserRunning(0) || users.isUserRunning(10)
                || !users.isUserUnlockingOrUnlocked(0) || users.isUserUnlockingOrUnlocked(10)
                || !users.hasUserRestriction("no_install_apps", 0)
                || users.hasUserRestriction("no_install_apps", 10)) throw new AssertionError();
        dead[0] = true;
        for (Runnable action : new Runnable[] {() -> storage.isCeStoragePrepared(0),
                () -> users.isUserRunning(0), () -> permissions.onPackageInstalled(pkg, -1, builder.build(), 0)}) {
            try { action.run(); throw new AssertionError("dead owner accepted"); }
            catch (RuntimeException expected) {}
        }
    }
}
