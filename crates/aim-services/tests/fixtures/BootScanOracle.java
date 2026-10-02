package com.android.server;

public final class BootScanOracle {
    public static void main(String[] args) {
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
