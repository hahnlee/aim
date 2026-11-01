/* Android Open Source Project, Apache-2.0; android-16.0.0_r1.
 * Call-shape reproduction of RoleService.Stub's role reads, getOrCreateUserState,
 * RoleUserState.readFile and RoleServicePlatformHelperImpl.readFromLegacySettings.
 * Android permission/persistence boundaries are doubled; the package adapter is
 * production. This does not execute the complete original RoleService.
 */
package dev.aim.server;

import android.os.Binder;
import android.os.IInterface;
import android.os.RemoteException;
import java.lang.reflect.Proxy;
import java.util.List;

final class RoleIdentityTest {
    private static final String BROWSER = "android.app.role.BROWSER";
    private static final String TEST = "android.app.role.RESERVED_FOR_TESTING_PROFILE_GROUP_EXCLUSIVITY";
    private static final String MANAGE = "android.permission.MANAGE_ROLE_HOLDERS";
    private static void check(boolean value) { if (!value) throw new AssertionError(); }
    @SuppressWarnings("unchecked")
    private static <T> T owner(Class<T> type, java.lang.reflect.InvocationHandler calls) {
        return (T) Proxy.newProxyInstance(type.getClassLoader(), new Class<?>[] {type}, calls);
    }
    private static final class ColdRoles {
        private final PackageLocalOwners.Packages packages;
        private final boolean manage;
        private String browser;
        ColdRoles(PackageLocalOwners.Packages packages, boolean manage) {
            this.packages = packages; this.manage = manage;
        }
        private void enforceCallingOrSelfPermission(String permission) {
            check(Binder.getCallingUid() == 10194);
            check(permission.equals(MANAGE));
            if (!manage) throw new SecurityException(permission);
        }
        private void crossUser(int user) {
            if (user != Binder.getCallingUid() / 100000)
                throw new SecurityException("android.permission.INTERACT_ACROSS_USERS_FULL");
        }
        private ColdRoles getOrCreateUserState(int user) {
            // Both current and legacy roles.xml absent: original constructor ->
            // readFile -> getLegacyRoleState -> readFromLegacySettings -> browser.
            browser = packages.removeLegacyDefaultBrowserPackageName(user);
            return this;
        }
        boolean isRoleVisibleForTestAsUser(String role, int user) {
            enforceCallingOrSelfPermission(MANAGE);
            if (!TEST.equals(role)) throw new IllegalArgumentException(role);
            getOrCreateUserState(user);
            return false; // Original initial mRolesVisibleForTest is empty.
        }
        List<String> getRoleHoldersAsUser(String role, int user) {
            crossUser(user);
            enforceCallingOrSelfPermission(MANAGE);
            getOrCreateUserState(user);
            return BROWSER.equals(role) && browser != null ? List.of(browser) : List.of();
        }
        boolean isRoleAvailableAsUser(String role, int user) {
            crossUser(user);
            getOrCreateUserState(user);
            return BROWSER.equals(role) && browser != null;
        }
    }
    static void run() {
        int[] calls = {0};
        int[] failure = {0};
        var nativeOwner = owner(IPackageInternalHost.class, (proxy, method, args) -> {
            if (!method.getName().equals("removeLegacyDefaultBrowserPackageName"))
                throw new AssertionError(method);
            calls[0]++;
            if ((int)args[1] != 1000) throw new SecurityException(
                    "Package internal persistence requires system UID");
            check((int)args[0] == 0 && (int)args[2] == 123);
            check(Binder.getCallingUid() == 1000 && Binder.getCallingPid() == 123);
            if (failure[0] == 1) throw new RemoteException("persistence transport died");
            if (failure[0] == 2) throw new IllegalStateException("persistence failed");
            return "browser";
        });
        var endpoint = new Binder() {
            @Override public IInterface queryLocalInterface(String descriptor) { return nativeOwner; }
        };
        var host = owner(ISystemProcess.class, (proxy, method, args) -> {
            check(method.getName().equals("getPackageInternalHost"));
            return endpoint;
        });
        var packages = new PackageLocalOwners.Packages(
                owner(android.content.pm.IPackageManager.class, (p, m, a) -> { throw new AssertionError(m); }),
                () -> { throw new AssertionError(); }, () -> host);
        Binder.uid = 10194; Binder.pid = 987;
        try {
            // PermissionControllerApplication.onCreate's cold visibility query.
            check(!new ColdRoles(packages, true).isRoleVisibleForTestAsUser(TEST, 0));
            check(new ColdRoles(packages, true).getRoleHoldersAsUser(BROWSER, 0).equals(List.of("browser")));
            check(new ColdRoles(packages, false).isRoleAvailableAsUser(BROWSER, 0));
            check(calls[0] == 3 && Binder.uid == 10194 && Binder.pid == 987);
            for (Runnable denied : new Runnable[] {
                    () -> new ColdRoles(packages, false).isRoleVisibleForTestAsUser(TEST, 0),
                    () -> new ColdRoles(packages, false).getRoleHoldersAsUser(BROWSER, 0),
                    () -> new ColdRoles(packages, true).getRoleHoldersAsUser(BROWSER, 10)}) {
                try { denied.run(); throw new AssertionError("permission bypassed"); }
                catch (SecurityException expected) {}
                check(calls[0] == 3 && Binder.uid == 10194 && Binder.pid == 987);
            }
            for (int mode : new int[] {1, 2}) {
                failure[0] = mode;
                try { new ColdRoles(packages, true).isRoleVisibleForTestAsUser(TEST, 0);
                    throw new AssertionError("failure swallowed"); }
                catch (RuntimeException expected) {}
                check(Binder.uid == 10194 && Binder.pid == 987);
            }
        } finally { Binder.uid = 1000; Binder.pid = 123; }
    }
}
