package com.android.server.pm;

public final class AppIdsOracle {
    public static void main(String[] args) {
        if (args.length != 0 && args[0].equals("identity")) {
            com.android.internal.pm.parsing.pkg.PackageImpl pkg =
                (com.android.internal.pm.parsing.pkg.PackageImpl)
                    com.android.internal.pm.parsing.pkg.PackageImpl.forTesting("new");
            com.android.internal.pm.pkg.component.ParsedComponentImpl[] components = {
                new com.android.internal.pm.pkg.component.ParsedActivityImpl(),
                new com.android.internal.pm.pkg.component.ParsedActivityImpl(),
                new com.android.internal.pm.pkg.component.ParsedServiceImpl(),
                new com.android.internal.pm.pkg.component.ParsedProviderImpl(),
                new com.android.internal.pm.pkg.component.ParsedPermissionImpl(),
                new com.android.internal.pm.pkg.component.ParsedPermissionGroupImpl(),
                new com.android.internal.pm.pkg.component.ParsedInstrumentationImpl(),
            };
            for (com.android.internal.pm.pkg.component.ParsedComponentImpl c : components) {
                c.setName("new.Class");
                c.setPackageName("new");
                c.getComponentName(); // Populate the original cached ComponentName.
                if (c instanceof com.android.internal.pm.pkg.component.ParsedMainComponentImpl) {
                    ((com.android.internal.pm.pkg.component.ParsedMainComponentImpl) c).setProcessName("new:process");
                }
            }
            pkg.addActivity((com.android.internal.pm.pkg.component.ParsedActivity) components[0]);
            pkg.addReceiver((com.android.internal.pm.pkg.component.ParsedActivity) components[1]);
            pkg.addService((com.android.internal.pm.pkg.component.ParsedService) components[2]);
            pkg.addProvider((com.android.internal.pm.pkg.component.ParsedProvider) components[3]);
            pkg.addPermission((com.android.internal.pm.pkg.component.ParsedPermission) components[4]);
            pkg.addPermissionGroup((com.android.internal.pm.pkg.component.ParsedPermissionGroup) components[5]);
            pkg.addInstrumentation((com.android.internal.pm.pkg.component.ParsedInstrumentation) components[6]);
            pkg.setPackageName("old");
            System.out.println(pkg.getPackageName() + " " + pkg.getManifestPackageName());
            for (com.android.internal.pm.pkg.component.ParsedComponentImpl c : components) {
                String process = c instanceof com.android.internal.pm.pkg.component.ParsedMainComponentImpl
                    ? ((com.android.internal.pm.pkg.component.ParsedMainComponentImpl) c).getProcessName() : "-";
                System.out.println(c.getPackageName() + " " + c.getName() + " " + process
                    + " " + c.getComponentName().getPackageName() + " " + c.getComponentName().getClassName());
            }
            return;
        }
        if (args.length != 0 && args[0].equals("oem")) {
            com.android.server.SystemConfig config = new com.android.server.SystemConfig(false);
            for (int i = 1; i < args.length; i++) {
                config.readPermissions(android.util.Xml.newPullParser(), new java.io.File(args[i]), 0);
            }
            for (java.util.Map.Entry<String, Integer> entry : config.getOemDefinedUids().entrySet()) {
                System.out.println(entry.getKey() + " " + entry.getValue());
            }
            return;
        }
        AppIdSettingMap ids = new AppIdSettingMap();
        SettingBase a = new SharedUserSetting("a", 1, 8);
        SettingBase b = new SharedUserSetting("b", 1, 8);
        System.out.println(ids.registerExistingAppId(10002, a, "a"));
        System.out.println(ids.acquireAndRegisterNewAppId(b));
        System.out.println(ids.acquireAndRegisterNewAppId(b));
        System.out.println(ids.acquireAndRegisterNewAppId(b));
        ids.removeSetting(10001);
        System.out.println(ids.acquireAndRegisterNewAppId(b));
        ids.replaceSetting(10001, a);
        System.out.println(ids.getSetting(10001) == a);
        ids.replaceSetting(1000, a);
        System.out.println(ids.getSetting(1000) == a);
        ids.removeSetting(10010);
        System.out.println(ids.acquireAndRegisterNewAppId(b));

        AppIdSettingMap restart = new AppIdSettingMap();
        restart.registerExistingAppId(10002, a, "a");
        System.out.println(restart.acquireAndRegisterNewAppId(b));
        AppIdSettingMap full = new AppIdSettingMap();
        int last = -1;
        for (int i = 0; i < 10000; i++) last = full.acquireAndRegisterNewAppId(a);
        System.out.println(last);
        System.out.println(full.acquireAndRegisterNewAppId(b));
        full.removeSetting(19999);
        System.out.println(full.acquireAndRegisterNewAppId(b));
    }
}
