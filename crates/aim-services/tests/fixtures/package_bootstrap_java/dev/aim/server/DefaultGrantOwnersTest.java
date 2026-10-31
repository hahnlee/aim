package dev.aim.server;
import android.content.pm.PackageManagerInternal;
import com.android.server.LocalServices;
import java.util.List;

final class DefaultGrantOwnersTest {
    @SuppressWarnings("unchecked")
    private static <T> T owner(Class<T> type, java.lang.reflect.InvocationHandler calls) {
        return (T) java.lang.reflect.Proxy.newProxyInstance(type.getClassLoader(), new Class<?>[] {type}, calls);
    }
    static void run() {
        for (boolean grantFirst : new boolean[] {false, true}) {
            for (Class<?> type : List.of(PackageManagerInternal.class,
                    com.android.server.pm.UserManagerInternal.class,
                    com.android.server.pm.permission.PermissionManagerServiceInternal.class,
                    android.os.storage.StorageManagerInternal.class,
                    com.android.server.pm.permission.LegacyPermissionManagerInternal.class)) LocalServices.removeServiceForTest(type);
            int before = LocalServices.packageRegistrations;
            android.os.IUserManager.Stub.service = owner(android.os.IUserManager.class, (proxy, method, args) -> {
                if (!method.getName().equals("getUsers")) throw new AssertionError(method);
                return List.of(new android.content.pm.UserInfo(0));
            });
            android.content.pm.IPackageManager.Stub.service = owner(android.content.pm.IPackageManager.class, (proxy, method, args) -> {
                if (!method.getName().equals("getInstalledPackages")) throw new AssertionError(method);
                return new android.content.pm.ParceledListSlice<>(List.of());
            });
            var calls = new java.util.ArrayList<String>();
            boolean[] dead = {false};
            var queries = owner(IPackageDefaultGrantQueries.class, (proxy, method, args) -> {
                if (dead[0]) throw new android.os.RemoteException("grant query owner died");
                calls.add(method.getName());
                switch (method.getName()) {
                    case "getKnownPackageNames":
                        if ((int) args[0] != 1 || ((int) args[1] != 0 && (int) args[1] != 10)) throw new AssertionError();
                        return new String[] {"known"};
                    case "getDisabledSystemPackageName":
                        if (!args[0].equals("updated")) throw new AssertionError(); return "disabled";
                    case "isPlatformSigned": return args[0].equals("android");
                    case "isResolveActivityComponent": return args[0].equals("resolver") && args[1].equals("Resolver");
                    default: throw new AssertionError(method);
                }
            });
            var context = new android.content.Context();
            var backend = owner(ISystemProcess.class, (proxy, method, args) -> { throw new AssertionError(method); });
            var bridge = new PackageBootstrapBridge();
            if (grantFirst) bridge.grantDefaultPermissions(queries, 0);
            else PackageLocalOwners.publish(backend, context, new com.android.server.pm.Installer(context));
            var legacy = LocalServices.getService(com.android.server.pm.permission.LegacyPermissionManagerInternal.class);
            int[] providerCalls = {0};
            legacy.setSyncAdapterPackagesProvider((authority, user) -> {
                if (!authority.equals("calendar") || (user != 0 && user != 10)) throw new AssertionError();
                providerCalls[0]++; return new String[]{"sync.adapter"};
            });
            var retained = LocalServices.getService(PackageManagerInternal.class);
            if (grantFirst) PackageLocalOwners.publish(backend, context, new com.android.server.pm.Installer(context));
            else bridge.grantDefaultPermissions(queries, 0);
            bridge.grantDefaultPermissions(queries, 10);
            if (retained != LocalServices.getService(PackageManagerInternal.class)
                    || LocalServices.packageRegistrations != before + 1
                    || legacy != LocalServices.getService(com.android.server.pm.permission.LegacyPermissionManagerInternal.class)
                    || providerCalls[0] != (grantFirst ? 1 : 2) || retained.getDynamicCodeLogger() == null || calls.size() != 8) throw new AssertionError(calls);
            dead[0] = true;
            try { bridge.grantDefaultPermissions(queries, 0); throw new AssertionError("dead query accepted"); }
            catch (RuntimeException expected) {
                if (!expected.getMessage().contains("grant query owner died")) throw expected;
            }
        }
        LocalServices.removeServiceForTest(PackageManagerInternal.class);
    }
}
