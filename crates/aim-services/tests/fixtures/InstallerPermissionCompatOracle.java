package com.android.server.pm;

import com.android.internal.compat.IPlatformCompat;
import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

public final class InstallerPermissionCompatOracle {
    private static final int UID = 1_023_456;

    public static void main(String[] args) throws Exception {
        verify(new String[] {"disabled"}, new boolean[] {false}, false);
        verify(new String[] {"enabled"}, new boolean[] {true}, true);
        verify(new String[] {"enabled", "disabled"}, new boolean[] {true, false}, false);
        verify(new String[0], new boolean[0], true);
        verify(null, new boolean[0], true);
    }

    private static void verify(String[] packages, boolean[] changes, boolean expected)
            throws Exception {
        List<String> applications = new ArrayList<>();
        List<Integer> users = new ArrayList<>();
        var computer = (Computer) Proxy.newProxyInstance(
                InstallerPermissionCompatOracle.class.getClassLoader(),
                new Class<?>[] {Computer.class},
                (proxy, method, arguments) -> {
                    if (method.getName().equals("getPackagesForUid")) {
                        if ((int) arguments[0] != UID) throw new AssertionError("wrong UID");
                        return packages;
                    }
                    if (method.getName().equals("getApplicationInfo")) {
                        applications.add((String) arguments[0]);
                        if ((long) arguments[1] != 0) throw new AssertionError("wrong flags");
                        users.add((int) arguments[2]);
                        return null;
                    }
                    throw new AssertionError("unexpected Computer call: " + method);
                });
        int[] next = {0};
        var compat = (IPlatformCompat) Proxy.newProxyInstance(
                InstallerPermissionCompatOracle.class.getClassLoader(),
                new Class<?>[] {IPlatformCompat.class},
                (proxy, method, arguments) -> {
                    if (!method.getName().equals("isChangeEnabled"))
                        throw new AssertionError("unexpected compatibility call: " + method);
                    if ((long) arguments[0] != 150857253L)
                        throw new AssertionError("wrong compatibility change");
                    return changes[next[0]++];
                });
        boolean actual = NativePackageEffectsBridge.isInstallerPermissionRequired(
                computer, compat, UID, 10);
        if (actual != expected) throw new AssertionError("wrong compatibility result");
        List<String> expectedApplications = packages == null
                ? List.of() : Arrays.asList(packages);
        if (!applications.equals(expectedApplications))
            throw new AssertionError("wrong package traversal: " + applications);
        for (int user : users) if (user != 10) throw new AssertionError("wrong user: " + user);
        if (next[0] != changes.length) throw new AssertionError("short-circuited shared UID");
    }
}
