package com.android.server.pm;

public final class NewSettingOracle {
    public static void main(String[] args) throws Exception {
        if (args.length != 0 && args[0].equals("native-copy")) {
            nativeCopy();
            return;
        }
        if (args.length != 0 && args[0].equals("abi-lifecycle")) {
            abiLifecycle(java.nio.file.Files.readAllBytes(java.nio.file.Path.of(args[1])));
            return;
        }
        if (args.length != 0 && args[0].equals("scan-abi-package")) {
            var pkg = (com.android.internal.pm.parsing.pkg.PackageImpl)
                com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(
                    java.nio.file.Files.readAllBytes(java.nio.file.Path.of(args[1])));
            var helper = new PackageAbiHelperImpl();
            var abis = helper.derivePackageAbi(pkg, true, false, null, new java.io.File("/data/app-lib")).first;
            abis.applyTo(pkg);
            if (abis.primary == null) {
                abis = helper.getBundledAppAbis(pkg); abis.applyTo(pkg);
            }
            var paths = helper.deriveNativeLibraryPaths(pkg, true, false, new java.io.File("/data/app-lib"));
            System.out.println(abis.primary); System.out.println(abis.secondary);
            System.out.println(paths.nativeLibraryRootDir); System.out.println(paths.nativeLibraryRootRequiresIsa);
            System.out.println(paths.nativeLibraryDir); System.out.println(paths.secondaryNativeLibraryDir);
            return;
        }
        if (args.length != 0 && args[0].equals("abi-package")) {
            var pkg = com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(
                java.nio.file.Files.readAllBytes(java.nio.file.Path.of(args[1])));
            var abis = new PackageAbiHelperImpl().derivePackageAbi(pkg,
                true, false, null, new java.io.File("/data/app-lib")).first;
            System.out.println(abis.primary); System.out.println(abis.secondary);
            return;
        }
        if (args.length != 0 && args[0].equals("abi-selection")) {
            abiSelection(java.nio.file.Files.readAllBytes(java.nio.file.Path.of(args[1])));
            return;
        }
        if (args.length != 0 && args[0].equals("zip-package")) {
            var pkg = com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(
                java.nio.file.Files.readAllBytes(java.nio.file.Path.of(args[1])));
            try (var handle = com.android.server.pm.parsing.pkg.AndroidPackageUtils.createNativeLibraryHandle(pkg)) {
                System.out.println(com.android.internal.content.NativeLibraryHelper.findSupportedAbi(
                    handle, args[2].equals("-") ? new String[] {} : args[2].split(",")));
                System.out.println(com.android.internal.content.NativeLibraryHelper.hasRenderscriptBitcode(handle));
            }
            return;
        }
        if (args.length != 0 && args[0].equals("zip-abis")) {
            zipAbis();
            return;
        }
        if (args.length != 0 && args[0].equals("shared-abis")) {
            sharedAbis(java.nio.file.Files.readAllBytes(java.nio.file.Path.of(args[1])));
            return;
        }
        if (args.length != 0 && args[0].equals("bundled-abis")) {
            var pkg = com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(
                java.nio.file.Files.readAllBytes(java.nio.file.Path.of(args[1])));
            var abis = new PackageAbiHelperImpl().getBundledAppAbis(pkg);
            System.out.println(abis.primary);
            System.out.println(abis.secondary);
            return;
        }
        if (args.length != 0 && args[0].equals("native-paths")) {
            var pkg = (com.android.internal.pm.parsing.pkg.PackageImpl)
                com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(
                    java.nio.file.Files.readAllBytes(java.nio.file.Path.of(args[1])));
            pkg.setPath(args[4]); pkg.setBaseApkPath(args[5]);
            pkg.setPrimaryCpuAbi(args[6].equals("-") ? null : args[6]);
            pkg.setSecondaryCpuAbi(args[7].equals("-") ? null : args[7]);
            var paths = new PackageAbiHelperImpl().deriveNativeLibraryPaths(pkg,
                Boolean.parseBoolean(args[2]), Boolean.parseBoolean(args[3]),
                new java.io.File("/data/app-lib"));
            System.out.println(paths.nativeLibraryRootDir);
            System.out.println(paths.nativeLibraryRootRequiresIsa);
            System.out.println(paths.nativeLibraryDir);
            System.out.println(paths.secondaryNativeLibraryDir);
            return;
        }
        if (args.length != 0 && args[0].equals("group-flags")) {
            groupFlags();
            return;
        }
        if (args.length != 0 && args[0].equals("compat-call")) {
            byte[] bytes = java.nio.file.Files.readAllBytes(java.nio.file.Path.of(args[1]));
            var owner = android.os.ServiceManager.getService("platform_compat");
            android.os.Parcel parcel = android.os.Parcel.obtain(owner);
            android.os.Parcel reply = android.os.Parcel.obtain();
            try {
                parcel.unmarshall(bytes, 0, bytes.length);
                parcel.setDataPosition(0);
                if (!owner.transact(Integer.parseInt(args[2]), parcel, reply, 0)) {
                    throw new IllegalStateException("compatibility transaction rejected");
                }
                java.nio.file.Files.write(java.nio.file.Path.of(args[3]), reply.marshall());
            } finally {
                reply.recycle();
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

    private static PackageSetting member(String name, int flags, int privateFlags) {
        return Settings.createNewSetting(name, null, null, null, null,
            new java.io.File("/system/nonexistent/" + name), null, null, null,
            1L, flags, privateFlags, null, true, false, false, false, null,
            null, null, null, null, null, null, new java.util.UUID(0, 0), 36, null);
    }

    private static void zipAbis() throws Exception {
        String[][][] layouts = {
            {{"assets/file"}}, {{"lib/x86/libx.so"}}, {{"lib/arm64-v8a/gdbserver"}},
            {{"lib/x86/anything", "lib/arm64-v8a/libx.so"}}, {{"lib/unknown/libx.so"}},
            {{"lib/x86/sub/libx.so", "lib/x86/lib x.so", "lib/a/x"}},
            {{"lib/x86/lib+,-.=_x.so"}}, {{"lib/x86/lib\u00e9.so"}},
            {{"lib/x86/"}}, {{"file.bc"}}, {{"dir/file.bc"}}, {{"bad dir/file.bc"}},
            {{"lib/unknown/libx.so"}, {"lib/x86/libx.so"}},
            {{"lib/x86/libx.so"}, {"lib/arm64-v8a/libx.so"}},
            {{"assets/file"}, {"lib/arm64-v8a/libx.so", "nested/file.bc"}},
            {{"lib/x86/" + "x".repeat(4096)}}, {{"lib/x86/lib\u0000x.so"}}
        };
        String[][] supported = {{"arm64-v8a", "x86"}, {"x86", "arm64-v8a"}, {}, {"unknown"}};
        for (int i = 0; i < layouts.length; i++) {
            var paths = new java.util.ArrayList<String>();
            for (int split = 0; split < layouts[i].length; split++) {
                String path = "/data/local/tmp/abi-inventory-" + i + "-" + split + ".zip";
                try (var zip = new java.util.zip.ZipOutputStream(java.nio.file.Files.newOutputStream(java.nio.file.Path.of(path)))) {
                    for (String name : layouts[i][split]) {
                        zip.putNextEntry(new java.util.zip.ZipEntry(name));
                        zip.write(new byte[] {1, 2, 3}); zip.closeEntry();
                    }
                }
                paths.add(path);
            }
            System.out.println("case " + i + " " + String.join(",", paths));
            try (var handle = com.android.internal.content.NativeLibraryHelper.Handle.create(paths, false, false, false, false)) {
                for (String[] abis : supported) System.out.println(
                    com.android.internal.content.NativeLibraryHelper.findSupportedAbi(handle, abis));
                System.out.println(com.android.internal.content.NativeLibraryHelper.hasRenderscriptBitcode(handle));
            } catch (java.io.IOException error) {
                System.out.println("error");
            }
        }
    }

    private static void nativeCopy() throws Exception {
        var localTime = java.time.LocalDateTime.of(2025, 1, 2, 3, 4, 6);
        System.out.println("clock " + localTime.atZone(java.time.ZoneId.systemDefault()).toEpochSecond());
        int cases = 0;
        for (int archive = 0; archive < 6; archive++) {
            for (boolean extract : new boolean[] {false, true}) {
                for (boolean debug : new boolean[] {false, true}) {
                    for (boolean disabled : new boolean[] {false, true}) {
                        int id = cases++;
                        String name = archive == 4 ? "lib/arm64-v8a/wrap.sh" :
                            archive == 5 ? "lib/x86/libx.so" : "lib/arm64-v8a/libx.so";
                        String file = archive == 4 ? "wrap.sh" : "libx.so";
                        byte[] payload = "native payload".getBytes(java.nio.charset.StandardCharsets.US_ASCII);
                        int method = archive == 0 || archive >= 4 ? 8 : 0;
                        byte[] compressed = payload;
                        if (method == 8) {
                            var compressor = new java.util.zip.Deflater(6, true);
                            compressor.setInput(payload); compressor.finish();
                            byte[] buffer = new byte[128]; int size = compressor.deflate(buffer);
                            compressed = java.util.Arrays.copyOf(buffer, size); compressor.end();
                        }
                        int offset = archive == 2 ? 4096 : archive == 3 ? 16384 : 100;
                        byte[] names = name.getBytes(java.nio.charset.StandardCharsets.US_ASCII);
                        var crc = new java.util.zip.CRC32(); crc.update(payload);
                        int central = offset + compressed.length;
                        var zip = java.nio.ByteBuffer.allocate(central + 46 + names.length + 22)
                            .order(java.nio.ByteOrder.LITTLE_ENDIAN);
                        int when = (((2025 - 1980) << 9) | (1 << 5) | 2) << 16 | (3 << 11) | (4 << 5) | 3;
                        zip.putInt(0, 0x04034b50); zip.putShort(4, (short)20); zip.putShort(8, (short)method);
                        zip.putInt(10, when); zip.putInt(14, (int)crc.getValue());
                        zip.putInt(18, compressed.length); zip.putInt(22, payload.length);
                        zip.putShort(26, (short)names.length); zip.putShort(28, (short)(offset - 30 - names.length));
                        zip.position(30); zip.put(names); zip.position(offset); zip.put(compressed);
                        zip.putInt(central, 0x02014b50); zip.putShort(central + 4, (short)20); zip.putShort(central + 6, (short)20);
                        zip.putShort(central + 10, (short)method); zip.putInt(central + 12, when);
                        zip.putInt(central + 16, (int)crc.getValue()); zip.putInt(central + 20, compressed.length);
                        zip.putInt(central + 24, payload.length); zip.putShort(central + 28, (short)names.length);
                        zip.position(central + 46); zip.put(names);
                        int end = zip.position(); zip.putInt(end, 0x06054b50);
                        zip.putShort(end + 8, (short)1); zip.putShort(end + 10, (short)1);
                        zip.putInt(end + 12, 46 + names.length); zip.putInt(end + 16, central);
                        String path = "/data/local/tmp/native-copy-" + id + ".zip";
                        java.nio.file.Files.write(java.nio.file.Path.of(path), zip.array());
                        var directory = new java.io.File("/data/local/tmp/native-copy-original-" + id);
                        if (!directory.mkdir()) throw new java.io.IOException("native directory creation failed");
                        try (var handle = com.android.internal.content.NativeLibraryHelper.Handle.create(
                                java.util.List.of(path), false, extract, debug, disabled)) {
                            int first = com.android.internal.content.NativeLibraryHelper.copyNativeBinaries(handle, directory, "arm64-v8a");
                            var output = new java.io.File(directory, file);
                            Object key = output.exists() ? java.nio.file.Files.readAttributes(output.toPath(),
                                java.nio.file.attribute.BasicFileAttributes.class).fileKey() : null;
                            int second = com.android.internal.content.NativeLibraryHelper.copyNativeBinaries(handle, directory, "arm64-v8a");
                            boolean reused = false;
                            if (output.exists()) {
                                if (key == null) throw new IllegalStateException("original inode unavailable");
                                reused = key.equals(java.nio.file.Files.readAttributes(output.toPath(),
                                    java.nio.file.attribute.BasicFileAttributes.class).fileKey());
                            }
                            System.out.println("case " + id + " " + path + " " + first + " " + second + " "
                                + output.exists() + " " + output.lastModified() + " " + output.length() + " " + reused);
                        }
                    }
                }
            }
        }
    }

    private static void abiSelection(byte[] cache) throws Exception {
        int cases = 0;
        for (int archive : new int[] {0, 1, 2, 3, 9}) {
            for (boolean multi : new boolean[] {false, true}) {
                for (boolean prefer32 : new boolean[] {false, true}) {
                    for (int sdk : new int[] {34, 35}) {
                        for (String overrideAbi : new String[] {null, "x86", "arm64-v8a"}) {
                            for (boolean library : new boolean[] {false, true}) {
                                var pkg = (com.android.internal.pm.parsing.pkg.PackageImpl)
                                    com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(cache);
                                pkg.setBaseApkPath("/data/local/tmp/abi-inventory-" + archive + "-0.zip")
                                    .setSplitCodePaths(null).setMultiArch(multi).set32BitAbiPreferred(prefer32)
                                    .setTargetSdkVersion(sdk);
                                if (library) pkg.addLibraryName("fixture.library");
                                System.out.print("case " + cases++ + " ");
                                try {
                                    var abis = new PackageAbiHelperImpl().derivePackageAbi(pkg,
                                        true, false, overrideAbi, new java.io.File("/data/app-lib")).first;
                                    System.out.println(abis.primary + "," + abis.secondary);
                                } catch (PackageManagerException error) {
                                    System.out.println("error=" + error.error + "," + error.getMessage());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    private static void abiLifecycle(byte[] cache) throws Exception {
        int cases = 0;
        for (int mode = 0; mode < 8; mode++) {
            for (boolean system : new boolean[] {false, true}) {
                for (boolean updated : new boolean[] {false, true}) {
                    for (String requested : new String[] {null, "-", "arm64-v8a"}) {
                        var pkg = (com.android.internal.pm.parsing.pkg.PackageImpl)
                            com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(cache);
                        pkg.setPackageName("fixture").setPrimaryCpuAbi("x86").setSecondaryCpuAbi("x86_64");
                        String overrideAbi = PackageManagerServiceUtils.deriveAbiOverride(requested);
                        var helper = new PackageAbiHelperImpl();
                        if (mode != 6) {
                            if (mode >= 1 && mode <= 3) {
                                var derived = helper.derivePackageAbi(pkg, system, updated, overrideAbi, new java.io.File("/data/app-lib"));
                                derived.first.applyTo(pkg); derived.second.applyTo(pkg);
                                if (system && !updated && derived.first.primary == null) helper.getBundledAppAbis(pkg).applyTo(pkg);
                            } else if (mode != 5) {
                                pkg.setPrimaryCpuAbi("armeabi-v7a").setSecondaryCpuAbi("arm64-v8a");
                            }
                            helper.deriveNativeLibraryPaths(pkg, system, updated, new java.io.File("/data/app-lib")).applyTo(pkg);
                            if (mode == 7) pkg.setPrimaryCpuAbi(android.os.Build.SUPPORTED_64_BIT_ABIS[0]);
                        }
                        var setting = member("fixture", 0, 0).setCpuAbiOverride(overrideAbi)
                            .setPrimaryCpuAbi(com.android.server.pm.parsing.pkg.AndroidPackageUtils.getRawPrimaryCpuAbi(pkg))
                            .setSecondaryCpuAbi(com.android.server.pm.parsing.pkg.AndroidPackageUtils.getRawSecondaryCpuAbi(pkg))
                            .setLegacyNativeLibraryPath(pkg.getNativeLibraryRootDir());
                        System.out.println("case " + cases++);
                        System.out.println(setting.getPrimaryCpuAbiLegacy());
                        System.out.println(com.android.server.pm.parsing.pkg.AndroidPackageUtils.getRawSecondaryCpuAbi(pkg));
                        System.out.println(pkg.getNativeLibraryRootDir()); System.out.println(pkg.isNativeLibraryRootRequiresIsa());
                        System.out.println(pkg.getNativeLibraryDir()); System.out.println(pkg.getSecondaryNativeLibraryDir());
                        System.out.println(setting.getCpuAbiOverride()); System.out.println(setting.getLegacyNativeLibraryPath());
                    }
                }
            }
        }
    }

    private static void sharedAbis(byte[] cache) throws Exception {
        String[][] layouts = {{null, null, null}, {null, "armeabi-v7a", "arm64-v8a"},
            {"armeabi-v7a", "armeabi", null}, {"arm64-v8a", null, "x86"}};
        int cases = 0;
        for (String[] layout : layouts) {
            for (int scan = 0; scan < 5; scan++) {
                for (int loaded = 0; loaded < 3; loaded++) {
                    var group = new SharedUserSetting("group", 0, 0);
                    var parsed = new java.util.HashMap<String, com.android.internal.pm.parsing.pkg.PackageImpl>();
                    for (int i = 0; i < 3; i++) {
                        String name = new String[] {"a", "b", "c"}[i];
                        var setting = member(name, 0, 0).setPrimaryCpuAbi(layout[i])
                            .setSecondaryCpuAbi("x86_64");
                        if (loaded != 0) {
                            var pkg = (com.android.internal.pm.parsing.pkg.PackageImpl)
                                com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(cache);
                            pkg.setPackageName(name).setPrimaryCpuAbi(loaded == 1 ? null : "arm64-v8a");
                            setting.setPkg(pkg); parsed.put(name, pkg);
                        }
                        group.addPackage(setting);
                    }
                    com.android.internal.pm.parsing.pkg.PackageImpl scanned = null;
                    if (scan != 0) {
                        scanned = (com.android.internal.pm.parsing.pkg.PackageImpl)
                            com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(cache);
                        scanned.setPackageName(scan < 3 ? "new" : scan == 3 ? "a" : "b")
                            .setPrimaryCpuAbi(scan == 2 ? "arm64-v8a" : scan == 4 ? "armeabi-v7a" : null);
                    }
                    var order = new java.util.ArrayList<String>();
                    for (var setting : group.getPackageStates()) order.add(setting.getPackageName());
                    System.out.println("case " + cases++ + " " + String.join(",", order));
                    String abi = new PackageAbiHelperImpl().getAdjustedAbiForSharedUser(
                        group.getPackageStates(), scanned);
                    System.out.println(abi);
                    var changed = ScanPackageUtils.applyAdjustedAbiToSharedUser(group, scanned, abi);
                    System.out.println(scanned == null ? "absent" :
                        com.android.server.pm.parsing.pkg.AndroidPackageUtils.getRawPrimaryCpuAbi(scanned));
                    for (var state : group.getPackageStates()) {
                        var setting = (PackageSetting) state;
                        System.out.print(setting.getPackageName() + "=" + setting.getPrimaryCpuAbiLegacy() + ";");
                    }
                    System.out.println();
                    System.out.println(changed);
                    for (String name : order) {
                        System.out.print(name + "=" + (parsed.containsKey(name)
                            ? com.android.server.pm.parsing.pkg.AndroidPackageUtils.getRawPrimaryCpuAbi(parsed.get(name))
                            : "absent") + ";");
                    }
                    System.out.println();
                }
            }
        }
    }

    private static void groupState(SharedUserSetting group) {
        System.out.println(group.getFlags() + " " + group.getPrivateFlags());
    }

    private static void groupFlags() {
        for (int seed : new int[] {0, 64}) {
            for (int privateSeed : new int[] {0, 8}) {
                for (int update : new int[] {0, 2, 128}) {
                    for (int privateUpdate : new int[] {0, 16}) {
                        var group = new SharedUserSetting("group", seed, privateSeed);
                        var a = member("a", 1, 32);
                        var b = member("b", 128, 16);
                        groupState(group);
                        group.addPackage(a); groupState(group);
                        group.addPackage(a); groupState(group);
                        a.setFlags(update); a.setPrivateFlags(privateUpdate);
                        group.addPackage(a); groupState(group);
                        group.addPackage(b); groupState(group);
                        System.out.println(group.removePackage(a)); groupState(group);
                        System.out.println(group.removePackage(a)); groupState(group);
                        System.out.println(group.removePackage(b)); groupState(group);
                        group.addPackage(a); groupState(group);
                        System.out.println(group.removePackage(a)); groupState(group);
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
