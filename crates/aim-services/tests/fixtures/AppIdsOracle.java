package com.android.server.pm;

public final class AppIdsOracle {
    public static void main(String[] args) {
        if (args.length != 0 && (args[0].equals("trust") || args[0].equals("join") || args[0].equals("ancestry"))) {
            android.content.pm.SigningDetails[] cases = {
                android.content.pm.SigningDetails.UNKNOWN,
                details(new int[]{1}, null, null),
                details(new int[]{2}, null, null),
                details(new int[]{2}, new int[]{1, 2}, new int[]{1, 0}),
                details(new int[]{2}, new int[]{1, 2}, new int[]{0, 0}),
                details(new int[]{3}, new int[]{1, 2, 3}, new int[]{3, 8, 0}),
                details(new int[]{1, 2}, null, null),
                details(new int[]{2, 1}, null, null),
                details(new int[]{1, 3}, null, null),
            };
            if (args[0].equals("ancestry")) {
                cases = new android.content.pm.SigningDetails[]{
                    android.content.pm.SigningDetails.UNKNOWN,
                    details(new int[]{1}, null, null),
                    details(new int[]{2}, null, null),
                    details(new int[]{2}, new int[]{1, 2}, new int[]{3, 0}),
                    details(new int[]{3}, new int[]{1, 2, 3}, new int[]{0, 2, 0}),
                    details(new int[]{3}, new int[]{2, 3}, new int[]{8, 0}),
                    details(new int[]{3}, new int[]{4, 2, 3}, new int[]{3, 2, 0}),
                    details(new int[]{4}, new int[]{1, 2, 4}, new int[]{3, 2, 0}),
                    details(new int[]{1, 2}, null, null),
                    details(new int[]{2, 1}, null, null),
                    details(new int[]{1, 3}, null, null),
                    details(new int[]{3}, new int[]{3}, new int[]{0}),
                };
                for (int i = 0; i < cases.length; i++) {
                    for (int j = 0; j < cases.length; j++) {
                        System.out.println(i + " " + j + " " + cases[i].hasCommonAncestor(cases[j]));
                    }
                }
                return;
            }
            if (args[0].equals("join")) {
                for (int i = 0; i < cases.length; i++) {
                    for (int j = 0; j < cases.length; j++) {
                        for (int k = 0; k < cases.length; k++) {
                            SharedUserSetting group = new SharedUserSetting("group", 1, 8);
                            group.signatures.mSigningDetails = cases[j];
                            PackageSetting member = new PackageSetting("member", null,
                                new java.io.File("/data/app/member"), 0, 0,
                                new java.util.UUID(0, 1)).setSigningDetails(cases[k]);
                            group.addPackage(member);
                            for (int kind = 0; kind < 3; kind++) {
                                System.out.println(i + " " + j + " " + k + " " + kind + " "
                                    + PackageManagerServiceUtils.canJoinSharedUserId("candidate", cases[i], group, kind));
                            }
                        }
                    }
                }
                return;
            }
            for (int i = 0; i < cases.length; i++) {
                for (int j = 0; j < cases.length; j++) {
                    android.content.pm.SigningDetails candidate = cases[i], old = cases[j];
                    boolean update = candidate.checkCapability(old, 1) || old.checkCapability(candidate, 8);
                    for (int flag : new int[]{0, 1, 2, 3, 8, 31}) {
                        System.out.println(i + " " + j + " " + flag + " "
                            + candidate.checkCapability(old, flag) + " " + candidate.hasAncestor(old)
                            + " " + candidate.hasAncestorOrSelf(old) + " " + update + " "
                            + (update || old.hasAncestorOrSelf(candidate)));
                    }
                }
            }
            return;
        }
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
    private static android.content.pm.SigningDetails details(int[] current, int[] past, int[] flags) {
        android.content.pm.Signature[] signatures = new android.content.pm.Signature[current.length];
        for (int i = 0; i < current.length; i++) signatures[i] = new android.content.pm.Signature(new byte[]{(byte) current[i]});
        android.content.pm.Signature[] lineage = past == null ? null : new android.content.pm.Signature[past.length];
        if (lineage != null) {
            for (int i = 0; i < past.length; i++) {
                lineage[i] = new android.content.pm.Signature(new byte[]{(byte) past[i]});
                lineage[i].setFlags(flags[i]);
            }
        }
        return new android.content.pm.SigningDetails(signatures, 3, null, lineage);
    }

}
