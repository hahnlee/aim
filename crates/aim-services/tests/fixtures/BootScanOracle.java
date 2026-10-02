package com.android.server;

public final class BootScanOracle {
    public static void main(String[] args) {
        if (args.length == 1 && args[0].equals("factory-users")) {
            com.android.server.pm.PackageSetting cold = new com.android.server.pm.PackageSetting(
                "fixture", null, new java.io.File("/system/app/fixture"), 1, 0,
                new java.util.UUID(0, 0));
            System.out.println(cold.readUserState(0).getFirstInstallTimeMillis());
            cold.setFirstInstallTime(123, 0);
            com.android.server.pm.PackageSetting copied = new com.android.server.pm.PackageSetting(cold, false);
            cold.setFirstInstallTime(456, 0);
            System.out.println(copied.readUserState(0).getFirstInstallTimeMillis());
            System.out.println(cold.readUserState(0).getFirstInstallTimeMillis());
            return;
        }
        if (args.length == 1 && args[0].equals("strict-signatures")) {
            for (String name : new java.util.TreeSet<>(SystemConfig.getInstance()
                    .getPreinstallPackagesWithStrictSignatureCheck())) {
                System.out.println(name);
            }
            return;
        }
        System.out.println(com.android.server.pm.parsing.library.PackageBackwardCompatibility
            .bootClassPathContainsATB());
        android.content.res.Resources resources = android.content.res.Resources.getSystem();
        int id = resources.getIdentifier("config_stopSystemPackagesByDefault", "bool", "android");
        System.out.println(resources.getBoolean(id));
        for (String name : new java.util.TreeSet<>(SystemConfig.getInstance()
                .getInitialNonStoppedSystemPackages())) {
            System.out.println(name);
        }
    }
}
