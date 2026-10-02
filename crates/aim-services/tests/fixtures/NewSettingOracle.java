package com.android.server.pm;

public final class NewSettingOracle {
    public static void main(String[] args) throws Exception {
        if (args.length != 0 && args[0].equals("compat-info")) {
            byte[] bytes = java.nio.file.Files.readAllBytes(java.nio.file.Path.of(args[1]));
            android.os.Parcel parcel = android.os.Parcel.obtain();
            try {
                parcel.unmarshall(bytes, 0, bytes.length);
                parcel.setDataPosition(0);
                var info = android.content.pm.ApplicationInfo.CREATOR.createFromParcel(parcel);
                parcel.enforceNoDataAvail();
                var compat = com.android.internal.compat.IPlatformCompat.Stub.asInterface(
                    android.os.ServiceManager.getService("platform_compat"));
                System.out.println(compat.isChangeEnabled(133396946L, info));
            } finally {
                parcel.recycle();
            }
            return;
        }
        if (args.length != 0 && args[0].equals("library-policy")) {
            System.out.println(com.android.server.pm.parsing.library.PackageBackwardCompatibility
                .bootClassPathContainsATB());
            return;
        }
        if (args.length != 0 && args[0].equals("libraries")) {
            var pkg = (com.android.internal.pm.parsing.pkg.PackageImpl)
                com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(
                    java.nio.file.Files.readAllBytes(java.nio.file.Path.of(args[1])));
            pkg.setTargetSdkVersion(Integer.parseInt(args[2]));
            for (String name : args[6].split(",")) if (!name.equals("-")) pkg.addUsesLibrary(name);
            for (String name : args[7].split(",")) if (!name.equals("-")) pkg.addUsesOptionalLibrary(name);
            boolean system = Boolean.parseBoolean(args[3]);
            boolean onBcp = com.android.server.pm.parsing.library.PackageBackwardCompatibility
                .bootClassPathContainsATB();
            if (!system && !onBcp) {
                var compat = com.android.internal.compat.IPlatformCompat.Stub.asInterface(
                    android.os.ServiceManager.getService("platform_compat"));
                System.out.println(compat.isChangeEnabled(133396946L,
                    com.android.server.pm.parsing.pkg.AndroidPackageUtils.generateAppInfoWithoutState(pkg)));
            }
            com.android.server.pm.parsing.library.PackageBackwardCompatibility.modifySharedLibraries(
                pkg, system, Boolean.parseBoolean(args[4]));
            java.nio.file.Files.write(java.nio.file.Path.of(args[5]),
                com.android.server.pm.parsing.PackageCacher.toCacheEntryStatic(pkg));
            return;
        }
        if (args.length != 0 && args[0].equals("policy")) {
            var pkg = com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(
                java.nio.file.Files.readAllBytes(java.nio.file.Path.of(args[1])));
            ScanPackageUtils.applyPolicy(pkg, Integer.parseInt(args[2]), null,
                Boolean.parseBoolean(args[3]));
            java.nio.file.Files.write(java.nio.file.Path.of(args[4]),
                com.android.server.pm.parsing.PackageCacher.toCacheEntryStatic(pkg));
            return;
        }
        if (args.length != 0 && args[0].equals("time")) {
            var pkg = com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(
                java.nio.file.Files.readAllBytes(java.nio.file.Path.of(args[1])));
            System.out.println(PackageManagerServiceUtils.getLastModifiedTime(pkg));
            return;
        }
        if (args.length != 0 && args[0].equals("update")) {
            updates();
            return;
        }
        for (int flags : new int[] {0, 1, 0x40000000, 0x40000001}) {
            for (boolean shared : new boolean[] {false, true}) {
                for (boolean stopped : new boolean[] {false, true}) {
                    for (Integer user : new Integer[] {null, 0, 10, -1}) {
                        System.err.println("case " + flags + " " + shared + " " + stopped + " " + user);
                        SharedUserSetting group = null;
                        if (shared) {
                            group = new SharedUserSetting("group", 0, 0);
                            group.mAppId = 10000;
                        }
                        PackageSetting p = Settings.createNewSetting("fixture", null, null,
                            null, group, new java.io.File("/system/nonexistent/fixture"),
                            "/system/lib64", "arm64-v8a", null, 0x100000002L, flags, 8,
                            user == null ? null : android.os.UserHandle.of(user),
                            true, true, true, stopped, null, new String[] {"sdk"},
                            new long[] {9}, new boolean[] {true}, new String[] {"static"},
                            new long[] {7}, java.util.Set.of("mime"),
                            java.util.UUID.fromString("00010203-0405-0607-0809-0a0b0c0d0e0f"), 36,
                            new byte[] {1, 2});
                        if (!shared) p.setAppId(10000);
                        System.out.print(p.getAppId() + " " + p.hasSharedUser() + " "
                            + p.getCategoryOverride() + " " + p.getPageSizeAppCompatFlags() + " "
                            + p.getKeySetData().getProperSigningKeySet() + " " + p.isLoading()
                            + " " + p.getLoadingProgress() + " " + p.getDomainSetId()
                            + " " + p.isScannedAsStoppedSystemApp());
                        for (int id : new int[] {0, 10, -1}) {
                            System.out.print(" " + p.getInstalled(id) + " " + p.readUserState(id).isStopped()
                                + " " + p.readUserState(id).isNotLaunched() + " " + p.getInstantApp(id)
                                + " " + p.getVirtualPreload(id));
                        }
                        System.out.println();
                    }
                }
            }
        }
    }

    private static void updates() {
        for (int oldSystem : new int[] {0, 1}) {
            for (int newSystem : new int[] {0, 1}) {
                for (int required : new int[] {0, 512}) {
                    for (boolean changed : new boolean[] {false, true}) {
                        PackageSetting p = Settings.createNewSetting("fixture", null, null,
                            null, null, new java.io.File("/system/nonexistent/old"),
                            "old.lib", "old.abi", null, 7L, 64 | oldSystem, required,
                            null, true, false, false, false, null, null, null, null, null, null,
                            java.util.Set.of("keep", "remove"), new java.util.UUID(0, 0), 35, null);
                        p.setAppId(10000);
                        p.setInstalled(false, 0);
                        p.setUninstallReason(3, 0);
                        Settings.updatePackageSetting(p, null, null, null,
                            new java.io.File(changed ? "/system/nonexistent/new" : "/system/nonexistent/old"),
                            "new.lib", "new.abi", null, 128 | newSystem, 8 | (required ^ 512),
                            null, null, null, null, null, null, java.util.Set.of("keep", "new"),
                            new java.util.UUID(0, 1), 36, new byte[] {1}, false);
                        System.out.println(p.getAppId() + " " + p.hasSharedUser() + " "
                            + p.getFlags() + " " + p.getPrivateFlags() + " "
                            + p.getLegacyNativeLibraryPath() + " " + p.getPrimaryCpuAbiLegacy()
                            + " " + p.getVersionCode() + " " + p.getInstalled(0) + " "
                            + p.getUninstallReason(0) + " "
                            + new java.util.TreeSet<>(p.getMimeGroups().keySet()));
                    }
                }
            }
        }
    }
}
