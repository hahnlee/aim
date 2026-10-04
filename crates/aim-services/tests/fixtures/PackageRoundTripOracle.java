import com.android.server.pm.parsing.PackageCacher;

public final class PackageRoundTripOracle {
    public static void main(String[] args) throws Exception {
        try {
            verify(args);
        } catch (Throwable failure) {
            failure.printStackTrace(System.out);
            System.exit(1);
        }
    }

    private static android.content.pm.SharedLibraryInfo nullableLibrary() {
        return new android.content.pm.SharedLibraryInfo(null, "nullable.owner",
            java.util.Arrays.asList(null, "/system/nullable.apk"), "nullable.library", 51L, 1,
            new android.content.pm.VersionedPackage("nullable.owner", 53L),
            java.util.Arrays.asList(null, new android.content.pm.VersionedPackage("nullable.consumer", 59L)),
            java.util.Arrays.asList(null, new android.content.pm.SharedLibraryInfo(
                "/system/framework/nested.jar", null, null, "nullable.nested", -1L, 0,
                new android.content.pm.VersionedPackage("android", 0L), null, null, false)), false);
    }
    private static void verifyNullableLibrary(android.content.pm.SharedLibraryInfo library) {
        if (!library.getAllCodePaths().equals(java.util.Arrays.asList(null, "/system/nullable.apk"))
                || library.getDependentPackages().get(0) != null
                || !library.getDependentPackages().get(1).getPackageName().equals("nullable.consumer")
                || library.getDependencies().get(0) != null
                || !library.getDependencies().get(1).getName().equals("nullable.nested")) throw new AssertionError("nullable library elements differ");
    }

    private static void verifyLibraryOwners(java.io.File file) throws Exception {
        byte[] bytes = java.nio.file.Files.readAllBytes(file.toPath());
        var in = android.os.Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            var info = android.content.pm.ApplicationInfo.CREATOR.createFromParcel(in);
            if (in.dataAvail() != 0 || info.sharedLibraryInfos.size() != 3
                    || info.optionalSharedLibraryInfos.size() != 1) throw new AssertionError("library owner framing differs");
            for (int i = 0; i < 2; i++) {
                var library = info.sharedLibraryInfos.get(i);
                if (!library.getOptionalDependentPackages().isEmpty() || !library.getCertDigests().isEmpty()) throw new AssertionError("null/empty library getters differ");
            }
            verifyPopulatedLibrary(info.sharedLibraryInfos.get(2));
            verifyPopulatedLibrary(info.optionalSharedLibraryInfos.get(0));
            var owners = new java.util.ArrayList<com.android.server.pm.pkg.SharedLibrary>();
            owners.add(new com.android.server.pm.pkg.SharedLibraryWrapper(new android.content.pm.SharedLibraryInfo(
                null, "owner", java.util.List.of("/system/owner.apk"), "original.null", 1, 3,
                new android.content.pm.VersionedPackage("owner", 7L), null, null, false)));
            owners.add(new com.android.server.pm.pkg.SharedLibraryWrapper(new android.content.pm.SharedLibraryInfo(
                "/system/framework/owner.jar", null, java.util.List.of("/explicit/code.jar"), "original.empty", 2, 0,
                new android.content.pm.VersionedPackage("android", 0L), java.util.List.of(), java.util.List.of(), false)));
            for (var library : info.sharedLibraryInfos) owners.add(new com.android.server.pm.pkg.SharedLibraryWrapper(library));
            owners.add(new com.android.server.pm.pkg.SharedLibraryWrapper(new android.content.pm.SharedLibraryInfo(
                "sdk.placeholder", 37L, 3, java.util.List.of("sdk.certificate"))));
            owners.add(new com.android.server.pm.pkg.SharedLibraryWrapper(nullableLibrary()));
            info.sharedLibraryInfos.get(2).getDependencies().add(nullableLibrary());
            var ownerInfos = new java.util.ArrayList<android.content.pm.SharedLibraryInfo>();
            for (var owner : owners) ownerInfos.add(((com.android.server.pm.pkg.SharedLibraryWrapper)owner).getInfo());
            var state = libraryState("library.fixture", 19001, ownerInfos);
            var restored = state.getLibraries();
            if (restored.size() != 7) throw new AssertionError("restored original owner count differs");
            for (int i = 0; i < restored.size(); i++) verifyLibraryBytes(ownerInfos.get(i), restored.get(i));
            verifyPopulatedLibrary(restored.get(4));
            verifyNullableLibrary(restored.get(4).getDependencies().get(1));
            verifyNullableLibrary(restored.get(6));
            var setting = new com.android.server.pm.PackageSetting("library.fixture", null,
                new java.io.File("/system/library-fixture"), 0, 0, new java.util.UUID(1, 2));
            setting.setAppId(19001);
            dev.aim.server.PackageObjects.restoreLibraries(setting, state, 1);
            var settingInfos = ((com.android.server.pm.pkg.PackageState)setting).getSharedLibraryDependencies();
            for (int i = 0; i < settingInfos.size(); i++) verifyLibraryBytes(ownerInfos.get(i),
                ((com.android.server.pm.pkg.SharedLibraryWrapper)settingInfos.get(i)).getInfo());
            restored.get(4).getOptionalDependentPackages().clear(); restored.get(4).getCertDigests().clear();
            restored.get(4).getDependencies().clear();
            verifyPopulatedLibrary(state.getLibraries().get(4));
            var freshStateInfos = state.getLibraries();
            for (int i = 0; i < freshStateInfos.size(); i++) verifyLibraryBytes(ownerInfos.get(i), freshStateInfos.get(i));
            var feed = android.os.Parcel.obtain();
            try {
                dev.aim.server.PackageLibraryFeed.write(feed, owners);
                java.nio.file.Files.write(new java.io.File(file.getParentFile(), "library-feed-original.parcel").toPath(), feed.marshall());
            } finally { feed.recycle(); }

        } finally { in.recycle(); }
    }
    private static dev.aim.server.PackageLibraryState libraryState(String name, int uid,
            java.util.List<android.content.pm.SharedLibraryInfo> infos) {
        var libraries = android.os.Parcel.obtain(); var envelope = android.os.Parcel.obtain();
        try {
            libraries.writeInt(infos.size());
            for (var library : infos) { libraries.writeInt(1); library.writeToParcel(libraries, 0); }
            envelope.writeLong(1); envelope.writeString(name); envelope.writeInt(uid);
            envelope.writeStringArray(new String[0]); envelope.writeByteArray(libraries.marshall());
            envelope.setDataPosition(0);
            return dev.aim.server.PackageLibraryState.read(envelope);
        } finally { libraries.recycle(); envelope.recycle(); }
    }
    private static void verifyLibraryBytes(android.content.pm.SharedLibraryInfo expected,
            android.content.pm.SharedLibraryInfo actual) {
        var input = android.os.Parcel.obtain(); var output = android.os.Parcel.obtain();
        try {
            expected.writeToParcel(input, 0); actual.writeToParcel(output, 0);
            if (!java.util.Arrays.equals(input.marshall(), output.marshall())) throw new AssertionError("restored original library parcel differs");
        } finally { input.recycle(); output.recycle(); }
    }

    private static void verifyPopulatedLibrary(android.content.pm.SharedLibraryInfo library) {
        var optional = library.getOptionalDependentPackages();
        var digests = library.getCertDigests();
        if (optional.size() != 2 || optional.get(0) != null
                || !optional.get(1).getPackageName().equals("consumer") || optional.get(1).getLongVersionCode() != Long.MAX_VALUE
                || digests.size() != 2 || digests.get(0) != null || !digests.get(1).equals("digest")) throw new AssertionError("optional/certificate library owners lost");
        var nested = library.getDependencies().get(0);
        if (!nested.getOptionalDependentPackages().get(0).getPackageName().equals("nested.consumer")
                || nested.getOptionalDependentPackages().get(0).getLongVersionCode() != 23
                || !nested.getCertDigests().equals(java.util.List.of("nested.digest"))) throw new AssertionError("nested library owners lost");
    }

    private static void verifyMutableCode(byte[] bytes) throws Exception {
        var owner = (com.android.internal.pm.parsing.pkg.PackageImpl)PackageCacher.fromCacheEntryStatic(bytes);
        int category = owner.getCategory();
        byte[] before = PackageCacher.toCacheEntryStatic(owner);
        int changedCategory = category == 7 ? -1 : 7;
        owner.setCategory(changedCategory);
        byte[] changed = PackageCacher.toCacheEntryStatic(owner);
        var captured = (com.android.internal.pm.parsing.pkg.PackageImpl)PackageCacher.fromCacheEntryStatic(changed);
        if (java.util.Arrays.equals(before, changed) || captured.getCategory() != changedCategory) throw new AssertionError("same-owner category mutation was not captured");
        owner.setCategory(category);
        if (!java.util.Arrays.equals(before, PackageCacher.toCacheEntryStatic(owner))) throw new AssertionError("original category restoration changed other code owners");
    }

    private record FallbackRead(java.util.Map<String, Integer> categories, boolean malformed) {}

    /*
     * Copyright (C) 2017 The Android Open Source Project
     *
     * Licensed under the Apache License, Version 2.0 (the "License");
     * you may not use this file except in compliance with the License.
     * You may obtain a copy of the License at
     *
     *      http://www.apache.org/licenses/LICENSE-2.0
     *
     * Unless required by applicable law or agreed to in writing, software
     * distributed under the License is distributed on an "AS IS" BASIS,
     * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
     * See the License for the specific language governing permissions and
     * limitations under the License.
     */
    // Adapted from core/java/android/content/pm/FallbackCategoryProvider.java
    // at android-16.0.0_r1: StringReader supplies controlled text and the
    // result records the original NumberFormatException boundary.
    private static FallbackRead readFallbacks(String text) throws Exception {
        var values = new android.util.ArrayMap<String, Integer>();
        try (var reader = new java.io.BufferedReader(new java.io.StringReader(text))) {
            String line;
            while ((line = reader.readLine()) != null) {
                if (line.charAt(0) == '#') continue;
                String[] fields = line.split(",");
                if (fields.length == 2) values.put(fields[0], Integer.parseInt(fields[1]));
            }
            return new FallbackRead(values, false);
        } catch (NumberFormatException expected) {
            return new FallbackRead(values, true);
        }
    }

    private static void verifyFallbackParsing() throws Exception {
        var first = readFallbacks("# header\r\nfirst,1\rfirst,٣,\nignored,1,2\nminimum,-2147483648\nbad,2147483648\nlate,6\n");
        if (!first.malformed() || !first.categories().equals(java.util.Map.of("first", 3, "minimum", Integer.MIN_VALUE))) throw new AssertionError("original duplicate/partial fallback parsing differs");
        var second = readFallbacks(",7\ntrailing,2,,\nmissing,\nskip,,\nspace, 1\nlate,3");
        if (!second.malformed() || !second.categories().equals(java.util.Map.of("", 7, "trailing", 2))) throw new AssertionError("original split/whitespace fallback parsing differs");
        if (!readFallbacks("").categories().isEmpty()) throw new AssertionError("empty fallback resource differs");
        for (String text : new String[] { "\n", "\r", "\r\n", "package,1\n\n" }) {
            try { readFallbacks(text); throw new AssertionError("original blank fallback line accepted"); }
            catch (StringIndexOutOfBoundsException expected) { }
        }
        String key = "debug.aim.fallback.boolean";
        try {
            for (String value : new String[] { "1", "y", "yes", "on", "true" }) {
                android.os.SystemProperties.set(key, value);
                if (!android.os.SystemProperties.getBoolean(key, false)) throw new AssertionError("original property true value differs: " + value);
            }
            for (String value : new String[] { "0", "n", "no", "off", "false" }) {
                android.os.SystemProperties.set(key, value);
                if (android.os.SystemProperties.getBoolean(key, true)) throw new AssertionError("original property false value differs: " + value);
            }
            for (String value : new String[] { "TRUE", " true ", "", "invalid" }) {
                android.os.SystemProperties.set(key, value);
                if (android.os.SystemProperties.getBoolean(key, false) || !android.os.SystemProperties.getBoolean(key, true)) throw new AssertionError("original property default differs: " + value);
            }
        } finally { android.os.SystemProperties.set(key, ""); }
    }

    private static void verify(String[] args) throws Exception {
        verifyFallbackParsing();
        android.content.pm.FallbackCategoryProvider.loadFallbacks();
        for (String line : java.nio.file.Files.readAllLines(new java.io.File(args[0], "fallback-categories.txt").toPath())) {
            String[] fields = line.split("\t");
            if (android.content.pm.FallbackCategoryProvider.getFallbackCategory(fields[0]) != Integer.parseInt(fields[1])) throw new AssertionError("original fallback resource value differs: " + fields[0]);
        }
        verifyLibraryOwners(new java.io.File(args[0], "library-owners.parcel"));
        LegacyPermissionOracle.verify(new java.io.File(args[0]));
        var files = new java.io.File(args[0]).listFiles((dir, name) -> name.endsWith(".native"));
        if (files == null) throw new java.io.IOException("missing parcel inputs");
        java.util.Arrays.sort(files);
        for (var file : files) {
            var pkg = (com.android.internal.pm.parsing.pkg.PackageImpl)
                PackageCacher.fromCacheEntryStatic(java.nio.file.Files.readAllBytes(file.toPath()));
            if (file.getName().startsWith("scan-")) {
                verifyMutableCode(java.nio.file.Files.readAllBytes(file.toPath()));
                verifySnapshot(file, pkg.getPackageName(), pkg.getUid());
                int uid = Integer.parseInt(file.getName().substring(5, file.getName().indexOf('.')));
                var signing = pkg.getSigningDetails();
                if (pkg.getUid() != uid || signing.getSignatureSchemeVersion() != 3
                    || signing.getSignatures().length != 1 || signing.getPublicKeys().size() != 1
                    || (uid == 10000 && signing.getPastSigningCertificates().length != 2)) {
                    throw new AssertionError("native scan UID/signing was not finalized: " + file.getName());
                }
                byte[][] certificates = null;
                int[] capabilities = null;
                try (var in = new java.io.DataInputStream(new java.io.FileInputStream(file.getPath() + ".signing"))) {
                    int count = in.readInt();
                    if (count >= 0) {
                        certificates = new byte[count][];
                        capabilities = new int[count];
                        for (int i = 0; i < count; i++) {
                            certificates[i] = in.readNBytes(in.readInt());
                            capabilities[i] = in.readInt();
                        }
                    }
                    if (in.read() != -1) throw new AssertionError("trailing signing metadata");
                }
                var restored = dev.aim.server.PackageObjects.fromCache(
                    java.nio.file.Files.readAllBytes(file.toPath()), certificates, capabilities);
                var actual = restored.getSigningDetails();
                if (uid == 10000) {
                    if (!java.util.Arrays.equals(capabilities, new int[] {21, 23})) {
                        throw new AssertionError("pinned verified GSF lineage changed: " + java.util.Arrays.toString(capabilities));
                    }
                    for (int i = 0; i < capabilities.length; i++) {
                        var historical = actual.getPastSigningCertificates()[i];
                        if (historical.getFlags() != capabilities[i]
                            || signing.getPastSigningCertificates()[i].getFlags() != 0
                            || historical == signing.getPastSigningCertificates()[i]) {
                            throw new AssertionError("lineage flags or isolation lost");
                        }
                        if (i < capabilities.length - 1) {
                            var old = new android.content.pm.SigningDetails(
                                new android.content.pm.Signature[] {historical}, 3);
                            for (int mask : new int[] {1, 2, 4, 8, 16, 32, 15, 31}) {
                                if (actual.checkCapability(old, mask)
                                        != ((capabilities[i] & mask) == mask)) {
                                    throw new AssertionError("original capability predicate differs");
                                }
                            }
                            var revoked = dev.aim.server.PackageObjects.restoreSigning(signing,
                                certificates, new int[capabilities.length]);
                            if (revoked.checkCapability(old, 1)) throw new AssertionError("revocation lost");
                        }
                    }
                    rejects(signing, certificates, null);
                    rejects(signing, certificates, new int[0]);
                    rejects(signing, null, capabilities);
                    byte[][] wrong = certificates.clone();
                    wrong[0] = new byte[] {0};
                    rejects(signing, wrong, capabilities);
                    wrong[0] = null;
                    rejects(signing, wrong, capabilities);
                    capabilities[0] = 0;
                    certificates[0][0] ^= 1;
                    actual.getSignatures()[0].setFlags(123);
                    actual.getPublicKeys().clear();
                    if (actual.getPastSigningCertificates()[0].getFlags() != 21
                        || signing.getSignatures()[0].getFlags() != 0
                        || signing.getPublicKeys().size() != 1) {
                        throw new AssertionError("mutable signing inputs leaked");
                    }
                } else if (actual.getPastSigningCertificates() != null) {
                    throw new AssertionError("absent lineage became present");
                }
            }
            if (file.getName().contains("-true.native") && (pkg.getUid() != 19001
                || !"arm64-v8a".equals(pkg.getPrimaryCpuAbi())
                || !"/data/app/fixture/lib".equals(pkg.getNativeLibraryRootDir())
                || pkg.getPageSizeAppCompatFlags() != 8)) {
                throw new AssertionError("native scan metadata was not decoded: " + file.getName());
            }
            java.nio.file.Files.write(new java.io.File(file.getPath() + ".original").toPath(),
                PackageCacher.toCacheEntryStatic(pkg));
        }
        System.out.println("PARCELS " + files.length);
    }

    private static void rejects(android.content.pm.SigningDetails signing,
            byte[][] certificates, int[] capabilities) {
        try {
            dev.aim.server.PackageObjects.restoreSigning(signing, certificates, capabilities);
        } catch (IllegalArgumentException expected) {
            return;
        }
        throw new AssertionError("inconsistent lineage accepted");
    }

    private static void verifySnapshot(java.io.File file, String name, int uid) throws Exception {
        byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".snapshot").toPath());
        byte[] usageBytes = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".usage").toPath());
        byte[] seinfoBytes = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".boot-seinfo").toPath());
        byte[] signingBytes = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".saved-signing").toPath());
        verifyRuntime(file);
        var owner = new PageOwner(name, bytes, usageBytes, seinfoBytes, signingBytes);
        var stale = new PageOwner(name, bytes, usageBytes, seinfoBytes, signingBytes);
        owner.userState = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".user").toPath());
        owner.libraries = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".libraries").toPath());
        owner.transientState = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".transient").toPath());
        stale.transientState = owner.transientState;
        owner.hiddenApiPolicy = Integer.parseInt(new String(java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".hidden-policy").toPath()), java.nio.charset.StandardCharsets.UTF_8));
        stale.hiddenApiPolicy = owner.hiddenApiPolicy;
        stale.userState = owner.userState;
        owner.setting = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".setting").toPath());
        stale.setting = owner.setting;
        owner.user10 = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".user-10").toPath());
        owner.user11 = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".user-11").toPath());
        owner.user12 = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".user-12").toPath());
        owner.user13 = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".user-13").toPath());
        owner.user14 = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".user-14").toPath());
        stale.version = 2;
        try (var bad = new dev.aim.server.PackageScanLease(
                dev.aim.server.IPackageScanSnapshot.Stub.asInterface(stale))) {
            try { bad.getSetting(name, false); throw new AssertionError("wrong setting version accepted"); } catch (java.io.IOException expected) {}
            try {
                bad.getUserState(name, false, 0);
                throw new AssertionError("wrong user-state version accepted");
            } catch (java.io.IOException expected) {}
            try {
                bad.getCode(name, false);
                throw new AssertionError("wrong page version accepted");
            } catch (java.io.IOException expected) {}
            try {
                bad.getUsage(name);
                throw new AssertionError("wrong usage version accepted");
            } catch (java.io.IOException expected) {}
            try {
                bad.getSigningState(name, false);
                throw new AssertionError("wrong signing version accepted");
            } catch (java.io.IOException expected) {}
            try {
                bad.getSeInfo(name);
                throw new AssertionError("wrong seInfo version accepted");
            } catch (java.io.IOException expected) {}
        }
        // Force generated Proxy/Stub parcel framing using the original Binder and Parcel.
        var endpoint = dev.aim.server.IPackageScanSnapshot.Stub.asInterface(owner);
        var lease = new dev.aim.server.PackageScanLease(endpoint);
        owner.shortChunk = true;
        try {
            lease.getUserState(name, false, 0);
            throw new AssertionError("short user-state chunk accepted");
        } catch (java.io.IOException expected) {}
        owner.shortChunk = false;
        var userState = lease.getUserState(name, false, 0);
        var originalDefault = new com.android.server.pm.pkg.PackageUserStateImpl(new com.android.server.utils.WatchableImpl());
        if (userState.getEnabledComponents() != null || userState.getDisabledComponents() != null
                || originalDefault.getEnabledComponentsNoCopy() != null
                || originalDefault.getDisabledComponentsNoCopy() != null) {
            throw new AssertionError("default nullable component owners differ");
        }
        originalDefault.setEnabledComponents((android.util.ArraySet<String>) null);
        originalDefault.setDisabledComponents((android.util.ArraySet<String>) null);
        if (originalDefault.getEnabledComponentsNoCopy() == null
                || originalDefault.getDisabledComponentsNoCopy() == null
                || originalDefault.getEnabledComponentsNoCopy().size() != 0
                || originalDefault.getDisabledComponentsNoCopy().size() != 0) {
            throw new AssertionError("Settings null setters must initialize empty component owners");
        }
        if (userState.overlayPaths == null
                || !userState.overlayPaths.resourceDirs.equals(java.util.List.of("base-apk"))
                || !userState.overlayPaths.overlayPaths.equals(java.util.List.of("base", "base", "base-apk"))
                || userState.getLibraryOverlays().size() != 3
                || !userState.getLibraryOverlays().get(0).library.equals("B")
                || !userState.getLibraryOverlays().get(1).library.equals("Aa")
                || !userState.getLibraryOverlays().get(2).library.equals("BB")
                || !userState.getLabelIcons().get(0).label.equals("")
                || userState.getLabelIcons().get(0).icon != 0) {
            throw new AssertionError("captured runtime user inputs differ");
        }
        try { userState.overlayPaths.overlayPaths.add("mutated"); throw new AssertionError("mutable captured overlay paths"); }
        catch (UnsupportedOperationException expected) {}
        try { userState.getLibraryOverlays().clear(); throw new AssertionError("mutable captured libraries"); }
        catch (UnsupportedOperationException expected) {}
        try { userState.getLabelIcons().clear(); throw new AssertionError("mutable captured label icons"); }
        catch (UnsupportedOperationException expected) {}
        verifyUserReplica(lease, name, file);
        if (originalDefault.getSuspendParams() != null) throw new AssertionError("default suspension map is not null");
        originalDefault.setSuspendParams(new android.util.ArrayMap<android.content.pm.UserPackage, com.android.server.pm.pkg.SuspendParams>());
        if (originalDefault.getSuspendParams() == null || originalDefault.getSuspendParams().size() != 0) {
            throw new AssertionError("initialized empty suspension map differs");
        }
        int userReads = owner.userReads;
        if (userState.getVersion() != 1 || !userState.getPackageName().equals(name)
                || userState.getAppId() != uid || userState.getUserId() != 0 || userState.isFactory()
                || lease.getUserState(name, false, 0) != userState || owner.userReads != userReads
                || lease.getUserState(name, true, 0) != null || lease.getUserState("missing", false, 0) != null) {
            throw new AssertionError("captured user state identity or cache differs");
        }
        owner.fail = true;
        try {
            lease.getCode(name, false);
            throw new AssertionError("owner failure swallowed");
        } catch (android.os.RemoteException expected) {}
        owner.fail = false;
        owner.shortChunk = true;
        try {
            lease.getCode(name, false);
            throw new AssertionError("short chunk accepted");
        } catch (java.io.IOException expected) {}
        owner.shortChunk = false;
        var code = lease.getCode(name, false);
        int reads = owner.reads;
        if (lease.getCode(name, false) != code || owner.reads != reads
            || lease.getCode(name, true) != null || lease.getCode("missing", false) != null) {
            throw new AssertionError("capture cache or absent code differs");
        }
        var pkg = dev.aim.server.PackageObjects.fromSnapshot(code, 1, name);
        if (pkg.getUid() != uid || code.getVersion() != 1 || !code.getPackageName().equals(name)) {
            throw new AssertionError("captured package metadata differs");
        }
        if (uid == 10000 && !java.util.Arrays.equals(code.getCapabilities(), new int[] {21, 23})) {
            throw new AssertionError("transport lost lineage capabilities");
        }
        code.getCache()[0] ^= 1;
        if (code.getCertificates() != null) {
            code.getCertificates()[0][0] ^= 1;
            code.getCapabilities()[0] = 0;
        }
        var out = android.os.Parcel.obtain();
        try {
            code.writeToParcel(out, 0);
            if (!java.util.Arrays.equals(out.marshall(), bytes)) {
                throw new AssertionError("native/Java code DTO differs or getter mutation leaked");
            }
            java.nio.file.Files.write(new java.io.File(file.getPath() + ".snapshot.original").toPath(), out.marshall());
        } finally { out.recycle(); }
        try {
            dev.aim.server.PackageObjects.fromSnapshot(code, 2, name);
            throw new AssertionError("wrong capture accepted");
        } catch (IllegalArgumentException expected) {}
        try {
            dev.aim.server.PackageObjects.fromSnapshot(code, 1, "different");
            throw new AssertionError("wrong name accepted");
        } catch (IllegalArgumentException expected) {}
        owner.fail = true;
        try { lease.getUsage(name); throw new AssertionError("usage owner error swallowed"); }
        catch (android.os.RemoteException expected) {}
        owner.fail = false;
        owner.usageTail = true;
        try { lease.getUsage(name); throw new AssertionError("usage trailing bytes accepted"); }
        catch (java.io.IOException expected) {}
        owner.usageTail = false;
        try { lease.getUsage("alias"); throw new AssertionError("usage name mismatch accepted"); }
        catch (java.io.IOException expected) {}
        var usage = lease.getUsage(name);
        int usageReads = owner.usageReads;
        if (lease.getUsage(name) != usage || owner.usageReads != usageReads
                || lease.getUsage("missing") != null || usage.getVersion() != 1
                || !usage.isHistoricalAvailable()
                || !java.util.Arrays.equals(usage.getLastPackageUsageTimeInMills(), new long[]{-1,17,29,0,0,0,0,55})
                || usage.getLatestPackageUseTimeInMills() != 55
                || usage.getLatestForegroundPackageUseTimeInMills() != 29) {
            throw new AssertionError("captured usage differs");
        }
        usage.getLastPackageUsageTimeInMills()[0] = 99;
        var setting = new com.android.server.pm.PackageSetting(name, null,
                new java.io.File("/data/app/fixture"), 0, 0, new java.util.UUID(1, 1));
        setting.setAppId(uid);
        owner.fail = true;
        try { lease.getSetting(name, false); throw new AssertionError("setting failure swallowed"); } catch (android.os.RemoteException expected) {}
        owner.fail = false; owner.shortChunk = true;
        try { lease.getSetting(name, false); throw new AssertionError("short setting chunk accepted"); } catch (java.io.IOException expected) {}
        owner.shortChunk = false;
        var settingBytes = owner.setting;
        owner.setting = java.util.Arrays.copyOf(settingBytes, settingBytes.length + 4);
        try { lease.getSetting(name, false); throw new AssertionError("setting trailing bytes accepted"); } catch (java.io.IOException expected) {}
        owner.setting = settingBytes;
        owner.factorySetting = settingBytes;
        try { lease.getSetting(name, true); throw new AssertionError("setting scope mismatch accepted"); } catch (java.io.IOException expected) {}
        owner.factorySetting = null;
        var metadata = lease.getSetting(name, false);
        if (!metadata.hasLegacyPermissionState()) throw new AssertionError("missing captured legacy migration owner");
        if (!metadata.hasInstallPermissionsFixed() || metadata.isInstallPermissionsFixed() != name.equals("com.google.android.gsf")) throw new AssertionError("captured install permissions fixed differs");
        setting.setInstallPermissionsFixed(metadata.isInstallPermissionsFixed());
        if (setting.isInstallPermissionsFixed() != metadata.isInstallPermissionsFixed()) throw new AssertionError("original install permissions fixed getter differs");
        var copiedFixed = new com.android.server.pm.PackageSetting(setting, false);
        setting.setInstallPermissionsFixed(!setting.isInstallPermissionsFixed());
        if (copiedFixed.isInstallPermissionsFixed() != metadata.isInstallPermissionsFixed()) throw new AssertionError("install permissions fixed copy shares state");
        var truncatedFixed = java.util.Arrays.copyOf(settingBytes, settingBytes.length - 8);
        var truncatedParcel = android.os.Parcel.obtain();
        try {
            truncatedParcel.unmarshall(truncatedFixed, 0, truncatedFixed.length); truncatedParcel.setDataPosition(0);
            try { dev.aim.server.PackageSettingData.read(truncatedParcel); throw new AssertionError("missing fixed marker became false"); } catch (IllegalArgumentException expected) {}
        } finally { truncatedParcel.recycle(); }
        var invalidFixed = settingBytes.clone();
        java.util.Arrays.fill(invalidFixed, invalidFixed.length - 8, invalidFixed.length - 4, (byte) 0);
        invalidFixed[invalidFixed.length - 8] = 2;
        var invalidParcel = android.os.Parcel.obtain();
        try {
            invalidParcel.unmarshall(invalidFixed, 0, invalidFixed.length); invalidParcel.setDataPosition(0);
            try { dev.aim.server.PackageSettingData.read(invalidParcel); throw new AssertionError("invalid fixed marker accepted"); } catch (IllegalArgumentException expected) {}
        } finally { invalidParcel.recycle(); }
        for (String suffix : new String[] {"true", "false", "unknown"}) {
            byte[] leavingBytes = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".setting-leaving-" + suffix).toPath());
            var leavingParcel = android.os.Parcel.obtain();
            try {
                leavingParcel.unmarshall(leavingBytes, 0, leavingBytes.length); leavingParcel.setDataPosition(0);
                var leavingData = dev.aim.server.PackageSettingData.read(leavingParcel);
                if (suffix.equals("unknown")) {
                    if (leavingData.hasLeavingSharedUser()) throw new AssertionError("unknown leaving bit became known");
                    try { com.android.server.pm.CapturedPackageSetting.from(leavingData, 1, false); throw new AssertionError("unknown leaving bit became false"); } catch (IllegalStateException expected) {}
                } else {
                    var restoredLeaving = com.android.server.pm.CapturedPackageSetting.from(leavingData, 1, false);
                    if (restoredLeaving.isLeavingSharedUser() != suffix.equals("true")) throw new AssertionError("original leaving getter differs");
                    var copiedLeaving = new com.android.server.pm.PackageSetting(restoredLeaving, false);
                    restoredLeaving.setLeavingSharedUser(!restoredLeaving.isLeavingSharedUser());
                    if (copiedLeaving.isLeavingSharedUser() != leavingData.isLeavingSharedUser()) throw new AssertionError("original leaving copy shares state");
                }
            } finally { leavingParcel.recycle(); }
        }
        for (boolean truncated : new boolean[] {true, false}) {
            byte[] malformed = java.util.Arrays.copyOf(settingBytes, settingBytes.length - (truncated ? 4 : 0));
            if (!truncated) { java.util.Arrays.fill(malformed, malformed.length - 4, malformed.length, (byte) 0); malformed[malformed.length - 4] = 2; }
            var malformedParcel = android.os.Parcel.obtain();
            try {
                malformedParcel.unmarshall(malformed, 0, malformed.length); malformedParcel.setDataPosition(0);
                try { dev.aim.server.PackageSettingData.read(malformedParcel); throw new AssertionError("malformed leaving marker accepted"); } catch (IllegalArgumentException expected) {}
            } finally { malformedParcel.recycle(); }
        }
        var legacy = metadata.getLegacyPermissionState();
        boolean populatedLegacy = name.equals("com.google.android.gsf");
        if (legacy.isMissing(10) != populatedLegacy || !legacy.getPermissionStates(0).isEmpty()
            || !legacy.getPermissionStates(11).isEmpty() || legacy.getPermissionStates(10).size() != (populatedLegacy ? 2 : 0)) throw new AssertionError("captured legacy migration users differ");
        if (populatedLegacy && (legacy.getPermissionState(null, 10).getFlags() != -1
            || !legacy.getPermissionState("BB", 10).isRuntime() || legacy.getPermissionState("BB", 10).isGranted()
            || legacy.getPermissionState("BB", 10).getFlags() != 17)) throw new AssertionError("captured legacy migration permissions differ");
        var legacySuffix = android.os.Parcel.obtain();
        byte[] unresolvedSetting;
        try {
            legacySuffix.writeBoolean(true); legacySuffix.writeIntArray(new int[] {10, 0, 11});
            legacySuffix.writeByteArray(dev.aim.server.PackageLegacyPermissions.capture(uid, new int[] {10, 0, 11}, legacy));
            legacySuffix.writeInt(metadata.isInstallPermissionsFixed() ? 1 : 0);
            legacySuffix.writeInt(metadata.isLeavingSharedUser() ? 1 : 0);
            int suffixLength = legacySuffix.marshall().length;
            unresolvedSetting = java.util.Arrays.copyOf(settingBytes, settingBytes.length - suffixLength + 12);
            java.util.Arrays.fill(unresolvedSetting, unresolvedSetting.length - 12, unresolvedSetting.length - 8, (byte) 0);
            java.util.Arrays.fill(unresolvedSetting, unresolvedSetting.length - 8, unresolvedSetting.length, (byte) -1);
        } finally { legacySuffix.recycle(); }
        var unresolvedParcel = android.os.Parcel.obtain();
        try {
            unresolvedParcel.unmarshall(unresolvedSetting, 0, unresolvedSetting.length); unresolvedParcel.setDataPosition(0);
            var unresolved = dev.aim.server.PackageSettingData.read(unresolvedParcel);
            if (unresolved.hasLegacyPermissionState() || unresolvedParcel.dataAvail() != 0) throw new AssertionError("unresolved legacy marker lost");
            if (unresolved.hasInstallPermissionsFixed()) throw new AssertionError("unresolved fixed marker lost");
            try { com.android.server.pm.CapturedPackageSetting.from(unresolved, 1, false); throw new AssertionError("unresolved setting became a replica"); } catch (IllegalStateException expected) {}
            try { unresolved.isInstallPermissionsFixed(); throw new AssertionError("unresolved fixed became false"); } catch (IllegalStateException expected) {}
            try { unresolved.getLegacyPermissionState(); throw new AssertionError("unresolved legacy became empty state"); } catch (IllegalStateException expected) {}
        } finally { unresolvedParcel.recycle(); }
        legacy.reset();
        if (metadata.getLegacyPermissionState().isMissing(10) != populatedLegacy) throw new AssertionError("mutable legacy owner escaped capture");
        com.android.server.pm.CapturedKeySetOracle.verifyCaptured(metadata, name.equals("com.google.android.gsf"));
        if (metadata != lease.getSetting(name, false) || metadata.appId != uid || metadata.getVersion() != 1
                || !metadata.getPackageName().equals(name) || metadata.isFactory()
                || metadata.loadingProgress != .5f || !metadata.isLoading() || metadata.loadingCompletedTime != 17
                || metadata.getOldPaths().size() != 3 || metadata.getOldPaths().get(1) != null
                || metadata.getOldPaths().get(2).length() != 100000) throw new AssertionError("captured setting metadata differs");
        metadata.getRestrictUpdateHash()[0] = 99;
        if (metadata.getRestrictUpdateHash()[0] != 1) throw new AssertionError("mutable setting hash escaped capture");
        try { metadata.getOldPaths().clear(); throw new AssertionError("mutable old paths capture"); } catch (UnsupportedOperationException expected) {}
        if (lease.getSetting("missing", false) != null || lease.getSetting(name, true) != null) throw new AssertionError("unknown/factory setting mismatch");
        var withUsers = lease.newSettingWithUsers(name, false, true);
        var userIds = new int[] {0, 10, 11, 12, 13, 14};
        if (withUsers.getUserStates().size() != userIds.length) throw new AssertionError("sparse inventory differs");
        for (int i = 0; i < userIds.length; i++) {
            int id = userIds[i];
            if (withUsers.getUserStates().keyAt(i) != id) throw new AssertionError("sparse ordering differs");
            var source = lease.getUserStateReplica(name, false, id, true);
            var target = withUsers.readUserState(id);
            if (target.getSharedLibraryOverlayPaths().getClass() != source.getSharedLibraryOverlayPaths().getClass()
                    || !target.getSharedLibraryOverlayPaths().equals(source.getSharedLibraryOverlayPaths())) throw new AssertionError("original watched overlay map differs: " + id);
            if (target.getCeDataInode() != source.getCeDataInode() || target.getDeDataInode() != source.getDeDataInode()
                    || target.getEnabledState() != source.getEnabledState() || target.isInstalled() != source.isInstalled()
                    || target.isStopped() != source.isStopped() || target.isNotLaunched() != source.isNotLaunched()
                    || target.isHidden() != source.isHidden() || target.getDistractionFlags() != source.getDistractionFlags()
                    || target.isInstantApp() != source.isInstantApp() || target.isVirtualPreload() != source.isVirtualPreload()
                    || target.getInstallReason() != source.getInstallReason() || target.getUninstallReason() != source.getUninstallReason()
                    || target.getFirstInstallTimeMillis() != source.getFirstInstallTimeMillis() || target.getMinAspectRatio() != source.getMinAspectRatio()
                    || !java.util.Objects.equals(target.getLastDisableAppCaller(), source.getLastDisableAppCaller())
                    || !java.util.Objects.equals(target.getHarmfulAppWarning(), source.getHarmfulAppWarning())
                    || !java.util.Objects.equals(target.getSplashScreenTheme(), source.getSplashScreenTheme())
                    || !target.getEnabledComponents().equals(source.getEnabledComponents()) || !target.getDisabledComponents().equals(source.getDisabledComponents())
                    || (target.getEnabledComponentsNoCopy() == null) != (source.getEnabledComponentsNoCopy() == null)
                    || (target.getDisabledComponentsNoCopy() == null) != (source.getDisabledComponentsNoCopy() == null)
                    || !java.util.Objects.equals(target.getOverlayPaths(), source.getOverlayPaths())
                    || !new java.util.LinkedHashMap<>(target.getSharedLibraryOverlayPaths()).equals(source.getSharedLibraryOverlayPaths())
                    || !java.util.Objects.equals(target.getAllOverlayPaths(), source.getAllOverlayPaths())
                    || (target.getSuspendParams() == null) != (source.getSuspendParams() == null)
                    || (target.getSuspendParams() != null && target.getSuspendParams().size() != source.getSuspendParams().size())
                    || (target.getArchiveState() == null) != (source.getArchiveState() == null)) throw new AssertionError("original assembled user differs: " + id);
        }
        var sealedLibraryMap = lease.getUserStateReplica(name, false, 10, true).getSharedLibraryOverlayPaths();
        try { sealedLibraryMap.clear(); throw new AssertionError("captured watched map is mutable"); } catch (IllegalStateException expected) {}
        var freshUsers = lease.newSettingWithUsers(name, false, true);
        withUsers.getOrCreateUserState(10).setStopped(false);
        withUsers.readUserState(10).getEnabledComponents().clear();
        withUsers.readUserState(10).getOverlayPaths().getOverlayPaths().clear();
        ((int[]) withUsers.readUserState(10).getSuspendParams().get(android.content.pm.UserPackage.of(0, "android")).getAppExtras().get("values"))[0] = 99;
        if (!freshUsers.readUserState(10).isStopped() || freshUsers.readUserState(10).getEnabledComponents().isEmpty()
                || freshUsers.readUserState(10).getOverlayPaths().getOverlayPaths().isEmpty()
                || ((int[]) freshUsers.readUserState(10).getSuspendParams().get(android.content.pm.UserPackage.of(0, "android")).getAppExtras().get("values"))[0] != 7) throw new AssertionError("assembled users share mutable state");
        if (!freshUsers.readUserState(999).isInstalled() || freshUsers.getUserStates().size() != 6
                || lease.newSettingWithUsers("missing", false, true) != null) throw new AssertionError("sparse original default differs");

        for (int[] ids : new int[][] {null, {-1}, {10, 10}, {10, 0}, {999}}) {
            owner.userInventory = ids;
            try { lease.newSettingWithUsers(name, false, true); throw new AssertionError("invalid sparse inventory accepted"); } catch (java.io.IOException expected) {}
        }
        owner.userInventory = new int[] {0, 10, 11, 12, 13, 14};
        owner.fail = true;
        try { lease.newSettingWithUsers(name, false, true); throw new AssertionError("inventory transport failure swallowed"); } catch (android.os.RemoteException expected) {}
        owner.fail = false;
        try { com.android.server.pm.CapturedPackageSetting.withUsers(metadata, java.util.List.of(userState, userState), 1, false, true); throw new AssertionError("duplicate user input accepted"); } catch (IllegalArgumentException expected) {}
        try { com.android.server.pm.CapturedPackageSetting.withUsers(metadata, java.util.List.of(userState), 2, false, true); throw new AssertionError("foreign user version accepted"); } catch (IllegalArgumentException expected) {}
        var restoredSetting = com.android.server.pm.CapturedPackageSetting.from(metadata, 1, false);
        var originalState = (com.android.server.pm.pkg.PackageState) restoredSetting;
        if (!java.util.Objects.equals(restoredSetting.getRealName(), metadata.realName)
                || !originalState.getPath().equals(new java.io.File(metadata.path))
                || restoredSetting.getFlags() != metadata.flags || restoredSetting.getPrivateFlags() != metadata.privateFlags
                || restoredSetting.getAppId() != metadata.appId || restoredSetting.hasSharedUser() != metadata.sharedUser
                || !java.util.Objects.equals(restoredSetting.getPrimaryCpuAbiLegacy(), metadata.primaryCpuAbiRaw)
                || !java.util.Objects.equals(restoredSetting.getSecondaryCpuAbiLegacy(), metadata.secondaryCpuAbiRaw)
                || !java.util.Objects.equals(restoredSetting.getCpuAbiOverride(), metadata.cpuAbiOverride)
                || originalState.getLastModifiedTime() != metadata.lastModifiedTime
                || originalState.getLastUpdateTime() != metadata.lastUpdateTime || originalState.getVersionCode() != metadata.versionCode
                || originalState.getTargetSdkVersion() != metadata.targetSdkVersion
                || originalState.getCategoryOverride() != metadata.categoryOverride
                || !java.util.Objects.equals(originalState.getVolumeUuid(), metadata.volumeUuid)
                || originalState.isUpdateAvailable() != metadata.updateAvailable
                || originalState.isForceQueryableOverride() != metadata.forceQueryable
                || originalState.isPendingRestore() != metadata.pendingRestore || originalState.isDebuggable() != metadata.debuggable
                || originalState.isScannedAsStoppedSystemApp() != metadata.scannedAsStoppedSystemApp
                || restoredSetting.getBaseRevisionCode() != metadata.baseRevisionCode
                || !java.util.Objects.equals(restoredSetting.getAppMetadataFilePath(), metadata.appMetadataFilePath)
                || restoredSetting.getAppMetadataSource() != metadata.appMetadataSource
                || !restoredSetting.getDomainSetId().equals(java.util.UUID.fromString(metadata.domainSetId))
                || !java.util.Arrays.equals(originalState.getRestrictUpdateHash(), metadata.getRestrictUpdateHash())
                || !restoredSetting.getOldPaths().equals(new java.util.LinkedHashSet<>(metadata.getOldPaths().stream().map(p -> p == null ? null : new java.io.File(p)).collect(java.util.stream.Collectors.toList())))) throw new AssertionError("original concrete metadata owner differs");
        var detachedSetting = com.android.server.pm.CapturedPackageSetting.from(metadata, 1, false);
        restoredSetting.getOldPaths().clear();
        restoredSetting.getLegacyPermissionState().reset();
        originalState.getRestrictUpdateHash()[0] = 99;
        if (detachedSetting.getOldPaths().size() != 3 || ((com.android.server.pm.pkg.PackageState)detachedSetting).getRestrictUpdateHash()[0] != 1
                || detachedSetting.getLegacyPermissionState().isMissing(10) != populatedLegacy) throw new AssertionError("original concrete replicas share mutable state");
        for (String suffix : new String[] {"null", "empty"}) {
            byte[] pathBytes = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".setting-" + suffix).toPath());
            var in = android.os.Parcel.obtain();
            try {
                in.unmarshall(pathBytes, 0, pathBytes.length); in.setDataPosition(0);
                var input = dev.aim.server.PackageSettingData.read(in);
                var replica = com.android.server.pm.CapturedPackageSetting.from(input, 1, false);
                if (in.dataAvail() != 0 || ("null".equals(suffix) ? replica.getOldPaths() != null : replica.getOldPaths() == null || !replica.getOldPaths().isEmpty())) throw new AssertionError("old path allocation lost");
            } finally { in.recycle(); }
        }
        try { com.android.server.pm.CapturedPackageSetting.from(metadata, 2, false); throw new AssertionError("foreign version accepted"); } catch (IllegalArgumentException expected) {}
        try { com.android.server.pm.CapturedPackageSetting.from(metadata, 1, true); throw new AssertionError("foreign scope accepted"); } catch (IllegalArgumentException expected) {}

        if (!java.util.Arrays.equals(restoredSetting.getUsesSdkLibraries(), metadata.getUsesSdkLibraries())
                || !java.util.Arrays.equals(restoredSetting.getUsesSdkLibrariesVersionsMajor(), metadata.getUsesSdkLibrariesVersionsMajor())
                || !java.util.Arrays.equals(restoredSetting.getUsesSdkLibrariesOptional(), metadata.getUsesSdkLibrariesOptional())
                || !java.util.Arrays.equals(restoredSetting.getUsesStaticLibraries(), metadata.getUsesStaticLibraries())
                || !java.util.Arrays.equals(restoredSetting.getUsesStaticLibrariesVersions(), metadata.getUsesStaticLibrariesVersions())
                || !restoredSetting.getMimeGroups().equals(metadata.getMimeGroups())) throw new AssertionError("captured collections differ from original getters");
        var mimeFeed = android.os.Parcel.obtain();
        try {
            dev.aim.server.PackageMimeGroups.write(mimeFeed, restoredSetting.getMimeGroups());
            try (var mimeOutput = new java.io.FileOutputStream(file.getPath() + ".mime-feed-original")) { mimeOutput.write(mimeFeed.marshall()); }
        } finally { mimeFeed.recycle(); }
        if (name.equals("com.google.android.gsf")) {
            if (!metadata.getUsesSdkLibraries()[0].equals("sdk") || metadata.getUsesSdkLibrariesVersionsMajor()[0] != Long.MAX_VALUE
                    || metadata.getUsesSdkLibrariesOptional()[0] || !metadata.getUsesStaticLibraries()[0].equals("static")
                    || metadata.getUsesStaticLibrariesVersions()[0] != 17 || metadata.getMimeGroups().get("BB").size() != 2) throw new AssertionError("populated captured collections differ");
            restoredSetting.getUsesSdkLibraries()[0] = "changed"; metadata.getUsesSdkLibraries()[0] = "changed";
            restoredSetting.getUsesSdkLibrariesVersionsMajor()[0] = 0; metadata.getUsesSdkLibrariesVersionsMajor()[0] = 0;
            restoredSetting.getUsesSdkLibrariesOptional()[0] = true; metadata.getUsesSdkLibrariesOptional()[0] = true;
            restoredSetting.getUsesStaticLibraries()[0] = "changed"; metadata.getUsesStaticLibraries()[0] = "changed";
            restoredSetting.getUsesStaticLibrariesVersions()[0] = 0; metadata.getUsesStaticLibrariesVersions()[0] = 0;
            restoredSetting.getMimeGroups().get("BB").clear();
            if (!metadata.getUsesSdkLibraries()[0].equals("sdk") || metadata.getUsesSdkLibrariesVersionsMajor()[0] != Long.MAX_VALUE
                    || metadata.getUsesSdkLibrariesOptional()[0] || !metadata.getUsesStaticLibraries()[0].equals("static")
                    || metadata.getUsesStaticLibrariesVersions()[0] != 17 || metadata.getMimeGroups().get("BB").size() != 2) throw new AssertionError("mutable original collection escaped capture");
            try { metadata.getMimeGroups().get("BB").clear(); throw new AssertionError("mutable captured MIME types"); } catch (UnsupportedOperationException expected) {}
        }
        try { metadata.getMimeGroups().clear(); throw new AssertionError("mutable captured MIME groups"); } catch (UnsupportedOperationException expected) {}
        if (name.equals("com.google.android.gsf")) {
            var expectedTypes = java.util.Arrays.asList(null, "", "BB", "Aa");
            if (!new java.util.ArrayList<>(metadata.getMimeGroups().get("nullable")).equals(expectedTypes)
                    || !new java.util.ArrayList<>(metadata.getMimeGroups().get(null)).equals(java.util.Arrays.asList(null, ""))) throw new AssertionError("nullable captured MIME order differs");
            var copy = new com.android.server.pm.PackageSetting(restoredSetting, false);
            copy.getMimeGroups().get("nullable").clear();
            if (!new java.util.ArrayList<>(restoredSetting.getMimeGroups().get("nullable")).equals(expectedTypes)) throw new AssertionError("original MIME copy shared its types");
            restoredSetting.getMimeGroups().get("nullable").clear();
            if (!new java.util.ArrayList<>(metadata.getMimeGroups().get("nullable")).equals(expectedTypes)) throw new AssertionError("original nullable types escaped capture");
            try { metadata.getMimeGroups().get(null).clear(); throw new AssertionError("mutable captured null group"); } catch (UnsupportedOperationException expected) {}
        }
        var nullableTypes = new android.util.ArraySet<String>(); nullableTypes.add(null);
        restoredSetting.addMimeTypes("nullable", nullableTypes);
        if (!restoredSetting.getMimeGroups().get("nullable").contains(null)) throw new AssertionError("original nullable MIME type owner differs");
        try { new java.util.TreeSet<String>(nullableTypes); throw new AssertionError("original feed TreeSet accepted null"); } catch (NullPointerException expected) {}
        if (name.equals("com.google.android.gsf")) {
            try { new java.util.TreeMap<String, java.util.Set<String>>(metadata.getMimeGroups()); throw new AssertionError("original feed TreeMap accepted null group"); } catch (NullPointerException expected) {}
        }
        restoredSetting.setLoadingProgress(metadata.loadingProgress); restoredSetting.setLoadingCompletedTime(metadata.loadingCompletedTime);
        for (String path : metadata.getOldPaths()) restoredSetting.addOldPath(path == null ? null : new java.io.File(path));
        if (!new java.util.ArrayList<>(restoredSetting.getOldPaths()).equals(metadata.getOldPaths().stream().map(path -> path == null ? null : new java.io.File(path)).toList())
                || restoredSetting.isLoading() != metadata.isLoading() || restoredSetting.getLoadingProgress() != metadata.loadingProgress
                || restoredSetting.getLoadingCompletedTime() != metadata.loadingCompletedTime) throw new AssertionError("original setting getters disagree");

        verifySettingRuntime(setting, file);
        verifyLoadingXml(file);
        com.android.server.pm.CapturedKeySetOracle.verify(file);
        verifyMimeXml(file);
        verifyNullableMimeWriter();
        var dynamic = new android.content.IntentFilter();
        try { dynamic.addDynamicDataType(null); throw new AssertionError("original dynamic MIME accepted null"); } catch (NullPointerException expected) {}
        try { dynamic.addDynamicDataType(""); throw new AssertionError("original dynamic MIME accepted empty"); } catch (android.content.IntentFilter.MalformedMimeTypeException expected) {}
        owner.fail = true;
        try { lease.getSigningState(name, false); throw new AssertionError("signing owner failure swallowed"); }
        catch (android.os.RemoteException expected) {}
        owner.fail = false;
        owner.signingTail = true;
        try { lease.getSigningState(name, false); throw new AssertionError("signing trailing bytes accepted"); }
        catch (java.io.IOException expected) {}
        owner.signingTail = false;
        try { lease.getSigningState("alias", false); throw new AssertionError("signing name mismatch accepted"); }
        catch (java.io.IOException expected) {}
        try { lease.getSigningState(name, true); throw new AssertionError("signing scope mismatch accepted"); }
        catch (java.io.IOException expected) {}
        var malformedSigning = android.os.Parcel.obtain();
        try {
            malformedSigning.writeLong(1); malformedSigning.writeString(name); malformedSigning.writeInt(uid);
            malformedSigning.writeBoolean(false); malformedSigning.writeString(null);
            malformedSigning.writeBoolean(true); malformedSigning.writeInt(3); malformedSigning.writeInt(1);
            malformedSigning.writeByteArray(new byte[]{3}); malformedSigning.writeInt(-1);
            malformedSigning.writeBoolean(false);
            owner.signingOverride = malformedSigning.marshall();
        } finally { malformedSigning.recycle(); }
        try { lease.getSigningState(name, false); throw new AssertionError("invalid signing certificate accepted"); }
        catch (IllegalArgumentException expected) {}
        owner.signingOverride = null;
        var savedSigning = lease.getSigningState(name, false);
        com.android.server.pm.CapturedInstallSourceOracle.verify(metadata, savedSigning.getPackageSigningDetails(), name.equals("com.google.android.gsf"));
        int signingReads = owner.signingReads;
        if (lease.getSigningState(name, false) != savedSigning || owner.signingReads != signingReads
                || lease.getSigningState("missing", false) != null || savedSigning.getAppId() != uid) {
            throw new AssertionError("saved signing lease identity");
        }
        var savedDetails = savedSigning.getPackageSigningDetails();
        var codeDetails = pkg.getSigningDetails();
        if (savedDetails.getSignatureSchemeVersion() != codeDetails.getSignatureSchemeVersion()
                || !java.util.Arrays.equals(savedDetails.getSignatures(), codeDetails.getSignatures())
                || !savedDetails.getPublicKeys().equals(codeDetails.getPublicKeys())) {
            throw new AssertionError("saved signing or derived public keys differ");
        }
        if (savedSigning.getSharedGroupName() != null) setting.setSharedUserAppId(uid);
        dev.aim.server.PackageObjects.restoreSavedSigning(setting, savedSigning, 1, false);
        var restored = setting.getSigningDetails();
        try {
            dev.aim.server.PackageObjects.restoreSavedSigning(setting, savedSigning, 2, false);
            throw new AssertionError("saved signing wrong version accepted");
        } catch (IllegalArgumentException expected) {}
        try {
            dev.aim.server.PackageObjects.restoreSavedSigning(setting, savedSigning, 1, true);
            throw new AssertionError("saved signing wrong scope accepted");
        } catch (IllegalArgumentException expected) {}
        setting.setAppId(uid + 1);
        try {
            dev.aim.server.PackageObjects.restoreSavedSigning(setting, savedSigning, 1, false);
            throw new AssertionError("saved signing wrong UID accepted");
        } catch (IllegalArgumentException expected) {}
        setting.setAppId(uid);
        if (savedSigning.getSharedGroupName() != null) {
            setting.setSharedUserAppId(uid + 1);
            try {
                dev.aim.server.PackageObjects.restoreSavedSigning(setting, savedSigning, 1, false);
                throw new AssertionError("saved signing wrong shared UID accepted");
            } catch (IllegalArgumentException expected) {}
            setting.setSharedUserAppId(uid);
        }
        if (setting.getSigningDetails() != restored) throw new AssertionError("rejected signing restore mutated setting");
        var unknownParcel = android.os.Parcel.obtain();
        try {
            unknownParcel.writeLong(1); unknownParcel.writeString(name); unknownParcel.writeInt(uid);
            unknownParcel.writeBoolean(false); unknownParcel.writeString(null);
            unknownParcel.writeBoolean(false); unknownParcel.writeBoolean(false);
            unknownParcel.setDataPosition(0);
            var unknown = dev.aim.server.PackageSigningState.CREATOR.createFromParcel(unknownParcel);
            if (unknown.getPackageSigningDetails() != android.content.pm.SigningDetails.UNKNOWN
                    || unknown.getSharedSigningDetails() != null) throw new AssertionError("unknown signing became known");
        } finally { unknownParcel.recycle(); }
        var signingOut = android.os.Parcel.obtain();
        try {
            savedSigning.writeToParcel(signingOut, 0);
            if (!java.util.Arrays.equals(signingOut.marshall(), signingBytes)) throw new AssertionError("native/Java saved signing DTO differs");
        } finally { signingOut.recycle(); }
        byte[] changedSigningBytes = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".saved-signing.changed").toPath());
        var changedOwner = new PageOwner(name, bytes, usageBytes, seinfoBytes, changedSigningBytes);
        changedOwner.version = 2;
        try (var changedLease = new dev.aim.server.PackageScanLease(dev.aim.server.IPackageScanSnapshot.Stub.asInterface(changedOwner))) {
            var changed = changedLease.getSigningState(name, false);
            if (uid == 10000) {
                var original = savedSigning.getPackageSigningDetails();
                var group = changed.getSharedSigningDetails();
                var current = changed.getPackageSigningDetails();
                if (original.getPastSigningCertificates()[0].getFlags() != 21
                        || current.getPastSigningCertificates()[0].getFlags() != 20
                        || group.getPastSigningCertificates()[0].getFlags() != 17
                        || !group.getPublicKeys().equals(current.getPublicKeys())) {
                    throw new AssertionError("package/group signing owners were conflated");
                }
                var ancestor = new android.content.pm.SigningDetails(
                    new android.content.pm.Signature[]{new android.content.pm.Signature(original.getPastSigningCertificates()[0])}, 3);
                if (current.checkCapability(ancestor, 1) || !original.checkCapability(ancestor, 1)) {
                    throw new AssertionError("saved signing capabilities lost");
                }
                original.getPastSigningCertificates()[0].setFlags(0);
                if (savedSigning.getPackageSigningDetails().getPastSigningCertificates()[0].getFlags() != 21) {
                    throw new AssertionError("saved signing DTO is mutable through original getters");
                }
            }
            dev.aim.server.PackageObjects.restoreSavedSigning(setting, changed, 2, false);
        }
        owner.fail = true;
        try { lease.getSeInfo(name); throw new AssertionError("seInfo owner failure swallowed"); }
        catch (android.os.RemoteException expected) {}
        owner.fail = false;
        owner.seinfoTail = true;
        try { lease.getSeInfo(name); throw new AssertionError("seInfo trailing bytes accepted"); }
        catch (java.io.IOException expected) {}
        owner.seinfoTail = false;
        try { lease.getSeInfo("alias"); throw new AssertionError("seInfo name mismatch accepted"); }
        catch (java.io.IOException expected) {}
        var security = lease.getSeInfo(name);
        int securityReads = owner.seinfoReads;
        if (lease.getSeInfo(name) != security || owner.seinfoReads != securityReads
                || lease.getSeInfo("missing") != null) throw new AssertionError("seInfo lease identity");
        setting.getPkgState().setSeInfo("before-base");
        dev.aim.server.PackageObjects.restoreBootSeInfo(setting, security, 1);
        if (!(security.isOverride() ? security.getLabel().equals(setting.getPkgState().getOverrideSeInfo())
                    && "before-base".equals(setting.getPkgState().getSeInfo())
                : security.getLabel().equals(setting.getPkgState().getSeInfo())
                    && setting.getPkgState().getOverrideSeInfo() == null)
                || !security.getLabel().equals(((com.android.server.pm.pkg.PackageState)setting).getSeInfo())) {
            throw new AssertionError("original boot base/override differs");
        }
        setting.getPkgState().setSeInfo("stale-base");
        setting.getPkgState().setOverrideSeInfo("stale-override");
        dev.aim.server.PackageObjects.restoreSeInfo(setting, security, 1);
        if (!java.util.Objects.equals(security.getBaseLabel(), setting.getPkgState().getSeInfo())
                || !java.util.Objects.equals(security.getOverrideLabel(), setting.getPkgState().getOverrideSeInfo())
                || !security.getLabel().equals(((com.android.server.pm.pkg.PackageState)setting).getSeInfo())) {
            throw new AssertionError("original complete seInfo fields differ");
        }
        android.os.Parcel incompleteParcel = android.os.Parcel.obtain();
        try {
            incompleteParcel.writeLong(1);
            incompleteParcel.writeString(name);
            incompleteParcel.writeString(null);
            incompleteParcel.writeString("boot-only-override");
            incompleteParcel.setDataPosition(0);
            var incomplete = dev.aim.server.PackageSeInfoState.CREATOR.createFromParcel(incompleteParcel);
            try { dev.aim.server.PackageObjects.restoreSeInfo(setting, incomplete, 1);
                throw new AssertionError("missing seInfo base fabricated"); }
            catch (IllegalStateException expected) {}
            if (!java.util.Objects.equals(security.getBaseLabel(), setting.getPkgState().getSeInfo())
                    || !java.util.Objects.equals(security.getOverrideLabel(), setting.getPkgState().getOverrideSeInfo())) {
                throw new AssertionError("incomplete seInfo restoration changed an owner");
            }
        } finally { incompleteParcel.recycle(); }
        android.os.Parcel securityParcel = android.os.Parcel.obtain();
        try {
            security.writeToParcel(securityParcel, 0);
            if (!java.util.Arrays.equals(seinfoBytes, securityParcel.marshall())) {
                throw new AssertionError("seInfo native/Java bytes differ");
            }
        } finally { securityParcel.recycle(); }
        try { dev.aim.server.PackageObjects.restoreBootSeInfo(setting, security, 2);
            throw new AssertionError("wrong seInfo version restored"); }
        catch (IllegalArgumentException expected) {}
        try { dev.aim.server.PackageObjects.restoreSeInfo(setting, security, 2);
            throw new AssertionError("wrong complete seInfo version restored"); }
        catch (IllegalArgumentException expected) {}
        dev.aim.server.PackageObjects.restoreUsage(setting, usage, 1);
        if (!java.util.Arrays.equals(setting.getPkgState().getLastPackageUsageTimeInMills(),
                usage.getLastPackageUsageTimeInMills())
                || setting.getPkgState().getLatestPackageUseTimeInMills() != usage.getLatestPackageUseTimeInMills()
                || setting.getPkgState().getLatestForegroundPackageUseTimeInMills() != usage.getLatestForegroundPackageUseTimeInMills()) {
            throw new AssertionError("original usage getters differ");
        }
        var securitySetting = new com.android.server.pm.PackageSetting(name, null,
                new java.io.File("/data/app/fixture"), 1, 8, new java.util.UUID(1, 3));
        boolean readPolicy = com.android.server.pm.SELinuxMMAC.readInstallPolicy();
        String expectedRead = new String(java.nio.file.Files.readAllBytes(
                new java.io.File(file.getPath() + ".seinfo-read").toPath()), java.nio.charset.StandardCharsets.UTF_8);
        if (!Boolean.toString(readPolicy).equals(expectedRead)) {
            throw new AssertionError("original/native policy load differs: original " + readPolicy + " native " + expectedRead);
        }
        String seinfo = com.android.server.pm.SELinuxMMAC.getSeInfo(
                (com.android.server.pm.pkg.PackageState)(Object)securitySetting, pkg, true, 36);
        String nativeSeinfo = new String(java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".seinfo").toPath()), java.nio.charset.StandardCharsets.UTF_8);
        if (!seinfo.equals(nativeSeinfo)) throw new AssertionError("native/original seinfo differs: " + nativeSeinfo + " vs " + seinfo);
        setting.getPkgState().setLastPackageUsageTimeInMills(0, 99);
        try { dev.aim.server.PackageObjects.restoreUsage(setting, usage, 2);
            throw new AssertionError("wrong usage version restored"); }
        catch (IllegalArgumentException expected) {}
        var other = new com.android.server.pm.PackageSetting("other", null,
                new java.io.File("/data/app/other"), 0, 0, new java.util.UUID(1, 2));
        try { dev.aim.server.PackageObjects.restoreBootSeInfo(other, security, 1);
            throw new AssertionError("wrong seInfo name restored"); }
        catch (IllegalArgumentException expected) {}
        try { dev.aim.server.PackageObjects.restoreSeInfo(other, security, 1);
            throw new AssertionError("wrong complete seInfo name restored"); }
        catch (IllegalArgumentException expected) {}
        if (!security.getLabel().equals(((com.android.server.pm.pkg.PackageState)setting).getSeInfo())
                || other.getPkgState().getOverrideSeInfo() != null) {
            throw new AssertionError("rejected seInfo restoration changed an owner");
        }
        try { dev.aim.server.PackageObjects.restoreUsage(other, usage, 1);
            throw new AssertionError("wrong usage name restored"); }
        catch (IllegalArgumentException expected) {}
        if (usage.getLastPackageUsageTimeInMills()[0] != -1
                || setting.getPkgState().getLastPackageUsageTimeInMills()[0] != 99
                || other.getPkgState().getLatestPackageUseTimeInMills() != 0) {
            throw new AssertionError("usage mutation or rejected restore changed an owner");
        }
        out = android.os.Parcel.obtain();
        try {
            usage.writeToParcel(out, 0);
            if (!java.util.Arrays.equals(out.marshall(), usageBytes)) {
                throw new AssertionError("native/Java usage DTO differs");
            }
        } finally { out.recycle(); }
        for (int length = 0; length <= owner.transientState.length; length++) {
            var malformedOwner = new PageOwner(name, bytes, usageBytes, seinfoBytes, signingBytes);
            malformedOwner.transientState = java.util.Arrays.copyOf(owner.transientState,
                length == owner.transientState.length ? length + 4 : length);
            try (var malformedLease = new dev.aim.server.PackageScanLease(dev.aim.server.IPackageScanSnapshot.Stub.asInterface(malformedOwner))) {
                try { malformedLease.getTransientState(name, false); throw new AssertionError("malformed transient transport accepted"); }
                catch (java.io.IOException | RuntimeException expected) {}
            }
        }
        CapturedTransientOracle.verify(file, metadata, lease);
        var transientOwner = new PageOwner(name, bytes, usageBytes, seinfoBytes, signingBytes);
        transientOwner.setting = owner.setting; transientOwner.libraries = owner.libraries;
        transientOwner.userState = owner.userState; transientOwner.user10 = owner.user10;
        transientOwner.user11 = owner.user11; transientOwner.user12 = owner.user12;
        transientOwner.user13 = owner.user13; transientOwner.user14 = owner.user14;
        transientOwner.transientState = java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".transient-23").toPath());
        try (var transientLease = new dev.aim.server.PackageScanLease(dev.aim.server.IPackageScanSnapshot.Stub.asInterface(transientOwner))) {
            var populated = transientLease.newScannedSetting(name, true);
            var state = (com.android.server.pm.pkg.PackageState)populated;
            if (!state.isHiddenUntilInstalled() || !state.isUpdatedSystemApp() || !state.isApkInUpdatedApex()
                    || !"com.example.apex".equals(state.getApexModuleName())) throw new AssertionError("combined transient setting differs");
            populated.getPkgState().setHiddenUntilInstalled(false).setUpdatedSystemApp(false)
                .setApkInUpdatedApex(false).setApexModuleName("changed");
            var freshState = (com.android.server.pm.pkg.PackageState)transientLease.newScannedSetting(name, true);
            if (!freshState.isHiddenUntilInstalled() || !freshState.isUpdatedSystemApp() || !freshState.isApkInUpdatedApex()
                    || !"com.example.apex".equals(freshState.getApexModuleName())) throw new AssertionError("combined transient setting shares mutable owner");
        }
        var replica = lease.getPackageStateReplica(name, true);
        if (replica != lease.getPackageStateReplica(name, true) || owner.hiddenApiReads != 1
                || lease.getPackageStateReplica("missing", true) != null) throw new AssertionError("captured PackageState identity differs");
        CapturedPackageStateOracle.verify(replica, lease.newScannedSetting(name, true));
        int capturedPolicy = owner.hiddenApiPolicy;
        owner.hiddenApiPolicy = capturedPolicy == 0 ? 2 : 0;
        if (replica.getHiddenApiEnforcementPolicy() != capturedPolicy
                || lease.getHiddenApiEnforcementPolicy(name, false) != capturedPolicy
                || owner.hiddenApiReads != 1) throw new AssertionError("policy capture follows later owner mutation");
        owner.hiddenApiPolicy = capturedPolicy;
        var invalidPolicyOwner = new PageOwner(name, bytes, usageBytes, seinfoBytes, signingBytes);
        invalidPolicyOwner.hiddenApiPolicy = 1;
        try (var invalidPolicyLease = new dev.aim.server.PackageScanLease(
                dev.aim.server.IPackageScanSnapshot.Stub.asInterface(invalidPolicyOwner))) {
            try { invalidPolicyLease.getHiddenApiEnforcementPolicy(name, false); throw new AssertionError("invalid hidden API policy accepted"); }
            catch (java.io.IOException expected) {}
        }
        var assembled = lease.newScannedSetting(name, true);
        var assembledState = (com.android.server.pm.pkg.PackageState)assembled;
        var assembledPkg = (com.android.internal.pm.parsing.pkg.PackageImpl)assembledState.getAndroidPackage();
        if (assembledPkg.getUid() != uid || !assembledPkg.getPackageName().equals(name)
                || assembledPkg.getLongVersionCode() != assembledState.getVersionCode()
                || !new java.io.File(assembledPkg.getPath()).equals(assembledState.getPath())
                || !java.util.Arrays.equals(assembled.getSigningDetails().getSignatures(), savedDetails.getSignatures())
                || !java.util.Arrays.equals(assembled.getPkgState().getLastPackageUsageTimeInMills(), usage.getLastPackageUsageTimeInMills())
                || !java.util.Objects.equals(assembled.getPkgState().getSeInfo(), security.getBaseLabel())
                || !java.util.Objects.equals(assembled.getPkgState().getOverrideSeInfo(), security.getOverrideLabel())
                || assembled.getUserStates().size() != 6) throw new AssertionError("combined captured setting differs");
        assembledPkg.setPackageName("mutated-code");
        assembled.getPkgState().setLastPackageUsageTimeInMills(0, 999);
        assembled.getOrCreateUserState(10).setStopped(false);
        var detached = lease.newScannedSetting(name, true);
        if (detached == assembled || ((com.android.server.pm.pkg.PackageState)detached).getAndroidPackage() == assembledPkg
                || !((com.android.internal.pm.parsing.pkg.PackageImpl)((com.android.server.pm.pkg.PackageState)detached).getAndroidPackage()).getPackageName().equals(name)
                || detached.getPkgState().getLastPackageUsageTimeInMills()[0] != -1
                || !detached.readUserState(10).isStopped()
                || lease.newScannedSetting("missing", true) != null) throw new AssertionError("combined setting shares mutable owners");
        var rejectedCode = com.android.server.pm.CapturedPackageSetting.from(metadata, 1, false);
        rejectedCode.setAppId(uid + 1);
        try { dev.aim.server.PackageObjects.restoreCollectedCode(rejectedCode, code, 1, false);
            throw new AssertionError("wrong code UID accepted"); } catch (IllegalArgumentException expected) {}
        rejectedCode.setAppId(uid).setLongVersionCode(metadata.versionCode + 1);
        try { dev.aim.server.PackageObjects.restoreCollectedCode(rejectedCode, code, 1, false);
            throw new AssertionError("wrong code version accepted"); } catch (IllegalArgumentException expected) {}
        if (((com.android.server.pm.pkg.PackageState)rejectedCode).getAndroidPackage() != null) throw new AssertionError("rejected code was attached");
        var wrongPath = new com.android.server.pm.PackageSetting(name, null, new java.io.File("/data/app/wrong"), 0, 0, new java.util.UUID(1, 2));
        wrongPath.setAppId(uid).setLongVersionCode(metadata.versionCode);
        try { dev.aim.server.PackageObjects.restoreCollectedCode(wrongPath, code, 1, false);
            throw new AssertionError("wrong code path accepted"); } catch (IllegalArgumentException expected) {}
        try { dev.aim.server.PackageObjects.restoreCollectedCode(detached, code, 2, false);
            throw new AssertionError("wrong code capture accepted"); } catch (IllegalArgumentException expected) {}
        var libraryState = lease.getLibraries(name);
        if (lease.getLibraries(name) != libraryState || lease.getLibraries("missing") != null
                || libraryState.getLibraries().get(0) == libraryState.getLibraries().get(0)) throw new AssertionError("library capture cache or detached object identity differs");
        var libraryInput = android.os.Parcel.obtain();
        var libraryOutput = android.os.Parcel.obtain();
        try {
            libraryInput.unmarshall(owner.libraries, 0, owner.libraries.length); libraryInput.setDataPosition(0);
            libraryInput.readLong(); libraryInput.readString(); libraryInput.readInt(); libraryInput.createStringArray();
            byte[] expectedLibraries = libraryInput.createByteArray();
            var decodedLibraries = libraryState.getLibraries();
            libraryOutput.writeInt(decodedLibraries.size());
            for (var library : decodedLibraries) { libraryOutput.writeInt(1); library.writeToParcel(libraryOutput, 0); }
            if (!java.util.Arrays.equals(expectedLibraries, libraryOutput.marshall())) throw new AssertionError("native/original library Parcelable differs");
            byte[] unreproducible = expectedLibraries.clone();
            java.util.Arrays.fill(unreproducible, unreproducible.length - 8, unreproducible.length - 4, (byte) 0xff);
            unreproducible[unreproducible.length - 8] = (byte)0xfe;
            libraryOutput.setDataPosition(0);
            var invalidLibraryEnvelope = android.os.Parcel.obtain();
            try {
                invalidLibraryEnvelope.writeLong(1); invalidLibraryEnvelope.writeString(name); invalidLibraryEnvelope.writeInt(uid);
                invalidLibraryEnvelope.writeStringArray(libraryState.getFiles().toArray(new String[0])); invalidLibraryEnvelope.writeByteArray(unreproducible);
                invalidLibraryEnvelope.setDataPosition(0);
                try { dev.aim.server.PackageLibraryState.read(invalidLibraryEnvelope); throw new AssertionError("malformed optional owner normalized"); } catch (IllegalArgumentException expected) {}
            } finally { invalidLibraryEnvelope.recycle(); }
        } finally { libraryInput.recycle(); libraryOutput.recycle(); }
        var badLibraryOwner = new PageOwner(name, bytes, usageBytes, seinfoBytes, signingBytes);
        badLibraryOwner.libraries = owner.libraries;
        try (var badLibraryLease = new dev.aim.server.PackageScanLease(dev.aim.server.IPackageScanSnapshot.Stub.asInterface(badLibraryOwner))) {
            badLibraryOwner.fail = true;
            try { badLibraryLease.getLibraries(name); throw new AssertionError("library owner error swallowed"); } catch (android.os.RemoteException expected) {}
            badLibraryOwner.fail = false; badLibraryOwner.shortChunk = true;
            try { badLibraryLease.getLibraries(name); throw new AssertionError("short library chunk accepted"); } catch (java.io.IOException expected) {}
            badLibraryOwner.shortChunk = false; badLibraryOwner.libraries = java.util.Arrays.copyOf(owner.libraries, owner.libraries.length + 4);
            try { badLibraryLease.getLibraries(name); throw new AssertionError("trailing library envelope accepted"); } catch (java.io.IOException expected) {}
            badLibraryOwner.libraries = owner.libraries;
            if (badLibraryLease.getLibraries(name) == null) throw new AssertionError("failed library reads poisoned retry");
        }
        var wrongLibraryUid = com.android.server.pm.CapturedPackageSetting.from(metadata, 1, false).setAppId(uid + 1);
        badLibraryOwner.version = 2;
        try (var wrongVersionLibraryLease = new dev.aim.server.PackageScanLease(dev.aim.server.IPackageScanSnapshot.Stub.asInterface(badLibraryOwner))) {
            try { wrongVersionLibraryLease.getLibraries(name); throw new AssertionError("wrong library capture version accepted"); } catch (java.io.IOException expected) {}
        }
        try { dev.aim.server.PackageObjects.restoreLibraries(wrongLibraryUid, libraryState, 1); throw new AssertionError("wrong library UID accepted"); } catch (IllegalArgumentException expected) {}
        var originalLibraries = ((com.android.server.pm.pkg.PackageState)detached).getSharedLibraryDependencies();
        if (originalLibraries.size() != 1 || !originalLibraries.get(0).getName().equals("aim.fixture")
                || !originalLibraries.get(0).getPath().equals(libraryState.getFiles().get(0))
                || !((com.android.server.pm.pkg.PackageState)detached).getUsesLibraryFiles().equals(libraryState.getFiles())) throw new AssertionError("original dependency getters differ");
        libraryState.getFiles().clear();
        libraryState.getLibraries().clear();
        var freshLibrarySetting = lease.newScannedSetting(name, true);
        if (((com.android.server.pm.pkg.PackageState)freshLibrarySetting).getSharedLibraryDependencies().size() != 1) throw new AssertionError("dependency getter mutation escaped input");
        var nullableEnvelope = android.os.Parcel.obtain();
        var nullableParcel = android.os.Parcel.obtain();
        try {
            nullableParcel.writeInt(1); nullableParcel.writeInt(1); nullableLibrary().writeToParcel(nullableParcel, 0);
            nullableEnvelope.writeLong(1); nullableEnvelope.writeString(name); nullableEnvelope.writeInt(uid);
            nullableEnvelope.writeStringArray(new String[0]); nullableEnvelope.writeByteArray(nullableParcel.marshall());
            nullableEnvelope.setDataPosition(0);
            var nullableState = dev.aim.server.PackageLibraryState.read(nullableEnvelope);
            verifyNullableLibrary(nullableState.getLibraries().get(0));
            var nullableSetting = com.android.server.pm.CapturedPackageSetting.from(metadata, 1, false);
            dev.aim.server.PackageObjects.restoreLibraries(nullableSetting, nullableState, 1);
            var restoredNullable = ((com.android.server.pm.pkg.SharedLibraryWrapper)
                ((com.android.server.pm.pkg.PackageState)nullableSetting).getSharedLibraryDependencies().get(0)).getInfo();
            verifyNullableLibrary(restoredNullable);
            restoredNullable.getAllCodePaths().set(0, "changed"); restoredNullable.getDependencies().clear();
            verifyNullableLibrary(nullableState.getLibraries().get(0));
        } finally { nullableEnvelope.recycle(); nullableParcel.recycle(); }
        var emptyLibraries = android.os.Parcel.obtain();
        try {
            emptyLibraries.writeLong(1); emptyLibraries.writeString(name); emptyLibraries.writeInt(uid);
            emptyLibraries.writeStringArray(new String[0]); emptyLibraries.writeByteArray(new byte[4]); emptyLibraries.setDataPosition(0);
            dev.aim.server.PackageObjects.restoreLibraries(freshLibrarySetting, dev.aim.server.PackageLibraryState.read(emptyLibraries), 1);
            var cleared = (com.android.server.pm.pkg.PackageState)freshLibrarySetting;
            if (!cleared.getSharedLibraryDependencies().isEmpty() || !cleared.getUsesLibraryFiles().isEmpty()) throw new AssertionError("empty dependency owner kept stale state");
        } finally { emptyLibraries.recycle(); }
        try { dev.aim.server.PackageObjects.restoreLibraries(detached, libraryState, 2); throw new AssertionError("wrong library version accepted"); } catch (IllegalArgumentException expected) {}
        for (int missing = 1; missing <= 6; missing++) {
            var incompleteOwner = new PageOwner(name, bytes, usageBytes, seinfoBytes, signingBytes);
            incompleteOwner.setting = owner.setting;
            incompleteOwner.libraries = owner.libraries;
            incompleteOwner.transientState = owner.transientState;
            incompleteOwner.userInventory = new int[0];
            incompleteOwner.missingOwner = missing;
            try (var incompleteLease = new dev.aim.server.PackageScanLease(dev.aim.server.IPackageScanSnapshot.Stub.asInterface(incompleteOwner))) {
                try { incompleteLease.newScannedSetting(name, true); throw new AssertionError("missing collected owner accepted: " + missing); }
                catch (java.io.IOException expected) {}
            }
        }
        lease.close();
        lease.close();
        if (owner.closes != 1) throw new AssertionError("close is not idempotent");
        try {
            lease.getCode(name, false);
            throw new AssertionError("closed lease accepted");
        } catch (IllegalStateException expected) {}
        try { lease.getUsage(name); throw new AssertionError("closed usage lease accepted"); }
        catch (IllegalStateException expected) {}
        try { lease.getSigningState(name, false); throw new AssertionError("closed signing lease accepted"); }
        catch (IllegalStateException expected) {}
        try { lease.getSeInfo(name); throw new AssertionError("closed seInfo lease accepted"); }
        catch (IllegalStateException expected) {}
        try { lease.getSetting(name, false); throw new AssertionError("closed setting lease accepted"); } catch (IllegalStateException expected) {}
        try { lease.newSettingWithUsers(name, false, true); throw new AssertionError("closed assembly lease accepted"); } catch (IllegalStateException expected) {}
        if (!replica.getPackageName().equals(name) || replica.getHiddenApiEnforcementPolicy() != owner.hiddenApiPolicy
                || replica.getTransientState().getLastPackageUsageTimeInMills()[0] != -1) throw new AssertionError("PackageState depends on closed lease");
        try { lease.getPackageStateReplica(name, true); throw new AssertionError("closed replica lease accepted"); } catch (IllegalStateException expected) {}
        try { lease.newScannedSetting(name, true); throw new AssertionError("closed scanned setting lease accepted"); } catch (IllegalStateException expected) {}
        try { lease.getLibraries(name); throw new AssertionError("closed dependency lease accepted"); } catch (IllegalStateException expected) {}
        try { lease.getUserStateReplica(name, false, 10, true); throw new AssertionError("closed user replica lease accepted"); }
        catch (IllegalStateException expected) {}
    }

    private static void settingRuntime(java.io.DataOutputStream out, com.android.server.pm.PackageSetting setting) throws Exception {
        out.writeInt(Float.floatToRawIntBits(setting.getLoadingProgress()));
        out.writeBoolean(setting.isLoading()); out.writeLong(setting.getLoadingCompletedTime());
        var paths = setting.getOldPaths(); out.writeInt(paths == null ? -1 : paths.size());
        if (paths != null) for (var path : paths) runtimeText(out, path == null ? null : path.toString());
    }
    private static void verifySettingRuntime(com.android.server.pm.PackageSetting setting, java.io.File file) throws Exception {
        try (var out = new java.io.DataOutputStream(new java.io.FileOutputStream(file.getPath() + ".setting-runtime.original"))) {
            settingRuntime(out, setting);
            for (float value : new float[] {-1f, -0f, 0f, Float.NaN, .25f, .1f, Math.nextDown(1f), 1f, Float.NaN, Math.nextUp(1f), 2f, Float.POSITIVE_INFINITY, Float.NEGATIVE_INFINITY}) {
                setting.setLoadingProgress(value); settingRuntime(out, setting);
            }
            for (long time : new long[] {-1, Long.MIN_VALUE, 123, Long.MAX_VALUE}) {
                setting.setLoadingCompletedTime(time); settingRuntime(out, setting);
            }
            setting.removeOldPath(null); settingRuntime(out, setting);
            setting.addOldPath(new java.io.File("/data/sole")); settingRuntime(out, setting);
            setting.removeOldPath(new java.io.File("/data/sole")); settingRuntime(out, setting);
            for (String path : new String[] {"//data//B/", "/data/B", "a/../b", "", null, "Aa", "BB"}) {
                setting.addOldPath(path == null ? null : new java.io.File(path)); settingRuntime(out, setting);
            }
            var copy = new com.android.server.pm.PackageSetting(setting, false);
            for (String path : new String[] {null, "missing", "/data/B/", "a/../b", "", "Aa", "BB"}) {
                setting.removeOldPath(path == null ? null : new java.io.File(path)); settingRuntime(out, setting);
            }
            settingRuntime(out, copy);
        }
    }

    private static void verifyLoadingXml(java.io.File file) throws Exception {
        try (var out = new java.io.DataOutputStream(new java.io.FileOutputStream(file.getPath() + ".loading-original"))) {
            for (int i = 0; i < 32; i++) {
                var setting = new com.android.server.pm.PackageSetting("fixture", null, new java.io.File("/data/app/fixture"), 0, 0, new java.util.UUID(1, 1));
                try (var input = new java.io.FileInputStream(file.getPath() + ".loading-" + i + ".xml")) {
                    var parser = android.util.Xml.resolvePullParser(input);
                    while (parser.next() != 2) {}
                    // Pinned Settings.readPackageLPw default getter and setter restoration.
                    if (i < 16) {
                        setting.setLoadingProgress(parser.getAttributeFloat(null, "loadingProgress", 0));
                        setting.setLoadingCompletedTime(parser.getAttributeLongHex(null, "loadingCompletedTime", 0));
                    } // readDisabledSysPackageLPw leaves constructor loading fields unchanged.
                }
                out.writeInt(Float.floatToRawIntBits(setting.getLoadingProgress()));
                out.writeBoolean(setting.isLoading()); out.writeLong(setting.getLoadingCompletedTime());
            }
        }
    }

    private static void verifyUserReplica(dev.aim.server.PackageScanLease lease, String name, java.io.File file) throws Exception {
        var originalArchive = new com.android.server.pm.pkg.ArchiveState(new java.util.ArrayList<>(java.util.List.of(
            new com.android.server.pm.pkg.ArchiveState.ArchiveActivityInfo("no icon", new android.content.ComponentName("fixture", "fixture.NoIcon"), null, java.nio.file.Path.of("/data/mono")),
            new com.android.server.pm.pkg.ArchiveState.ArchiveActivityInfo("icon", new android.content.ComponentName("fixture", "fixture.Icon"), java.nio.file.Path.of("/data/icon"), null))), "runtime installer", 123);
        var originalUser = new com.android.server.pm.pkg.PackageUserStateImpl(new com.android.server.utils.WatchableImpl());
        originalUser.setArchiveState(originalArchive);
        var runtimeArchive = lease.getUserStateReplica(name, false, 14, true);
        if (!originalUser.getArchiveState().equals(runtimeArchive.getArchiveState())) throw new AssertionError("nullable archive runtime getters differ");
        try { runtimeArchive.getArchiveState().getActivityInfos().clear(); throw new AssertionError("mutable captured archive list"); }
        catch (UnsupportedOperationException expected) {}
        originalArchive.getActivityInfos().clear();
        if (runtimeArchive.getArchiveState().getActivityInfos().size() != 2
                || runtimeArchive.getArchiveState().getActivityInfos().get(0).getIconBitmap() != null) throw new AssertionError("archive capture changed after original mutation");
        // Pinned Settings.writeArchiveStateLPr loop with original XML serializer.
        try (var output = new java.io.FileOutputStream(file.getPath() + ".null-icon.xml")) {
            var serializer = android.util.Xml.resolveSerializer(output);
            serializer.startDocument(null, true);
            serializer.startTag(null, "package-restrictions");
            serializer.startTag(null, "pkg"); serializer.attribute(null, "name", name);
            var archive = runtimeArchive.getArchiveState();
            serializer.startTag(null, "archive-state");
            serializer.attribute(null, "installer-title", archive.getInstallerTitle());
            serializer.attributeLongHex(null, "archive-time", archive.getArchiveTimeMillis());
            for (var activity : archive.getActivityInfos()) {
                serializer.startTag(null, "archive-activity-info");
                serializer.attribute(null, "activity-title", activity.getTitle());
                serializer.attribute(null, "original-component-name", activity.getOriginalComponentName().flattenToString());
                if (activity.getIconBitmap() != null) serializer.attribute(null, "icon-path", activity.getIconBitmap().toAbsolutePath().toString());
                if (activity.getMonochromeIconBitmap() != null) serializer.attribute(null, "monochrome-icon-path", activity.getMonochromeIconBitmap().toAbsolutePath().toString());
                serializer.endTag(null, "archive-activity-info");
            }
            serializer.endTag(null, "archive-state");
            serializer.endTag(null, "pkg"); serializer.endTag(null, "package-restrictions");
            serializer.endDocument();
        }
        for (int user : new int[] {12, 13}) {
            var original = new com.android.server.pm.pkg.PackageUserStateImpl(new com.android.server.utils.WatchableImpl());
            original.putSuspendParams(android.content.pm.UserPackage.of(0, "B"), user == 12 ? new com.android.server.pm.pkg.SuspendParams(null, null, null, true) : null);
            original.putSuspendParams(android.content.pm.UserPackage.of(0, "android"), user == 13 ? new com.android.server.pm.pkg.SuspendParams(null, null, null, true) : null);
            for (boolean policy : new boolean[] {false, true}) {
                var captured = lease.getUserStateReplica(name, false, user, policy);
                var nullKey = android.content.pm.UserPackage.of(0, user == 12 ? "android" : "B");
                if (!captured.isSuspended() || !original.isSuspended() || captured.getSuspendParams().size() != 2
                        || !captured.getSuspendParams().containsKey(nullKey) || captured.getSuspendParams().get(nullKey) != null) {
                    throw new AssertionError("explicit null runtime owner or absolute user key lost");
                }
                if (user == 12) {
                    try { original.isQuarantined(); throw new AssertionError("original null quarantine did not throw"); } catch (NullPointerException expected) {}
                    try { captured.isQuarantined(); throw new AssertionError("captured null quarantine did not throw"); } catch (NullPointerException expected) {}
                } else if (!original.isQuarantined() || !captured.isQuarantined()) throw new AssertionError("quarantine short circuit differs");
            }
            original.removeSuspension(android.content.pm.UserPackage.of(0, "B"));
            original.removeSuspension(android.content.pm.UserPackage.of(0, "android"));
            if (original.isSuspended() || original.getSuspendParams() == null || original.getSuspendParams().size() != 0) {
                throw new AssertionError("removal must retain allocated original map");
            }
        }
        // Pinned Settings writer loop emits a named empty tag for null params.
        var nil = new com.android.server.pm.pkg.PackageUserStateImpl(new com.android.server.utils.WatchableImpl());
        var key = android.content.pm.UserPackage.of(0, "android");
        nil.putSuspendParams(key, null);
        try (var output = new java.io.FileOutputStream(file.getPath() + ".null-params.xml")) {
            var serializer = android.util.Xml.resolveSerializer(output);
            serializer.startDocument(null, true);
            serializer.startTag(null, "package-restrictions");
            serializer.startTag(null, "pkg"); serializer.attribute(null, "name", name);
            serializer.startTag(null, "suspend-params");
            serializer.attribute(null, "suspending-package", key.packageName);
            serializer.attribute(null, "suspending-user", Integer.toString(key.userId));
            if (nil.getSuspendParams().get(key) != null) throw new AssertionError("null parameter unexpectedly materialized");
            serializer.endTag(null, "suspend-params");
            serializer.endTag(null, "pkg"); serializer.endTag(null, "package-restrictions");
            serializer.endDocument();
        }
        var state = lease.getUserStateReplica(name, false, 10, true);
        if (lease.getUserStateReplica(name, false, 10, true) != state) throw new AssertionError("replica identity differs");
        com.android.server.pm.pkg.PackageUserStateInternal internal = state;
        android.content.pm.pkg.FrameworkPackageUserState framework = state;
        if (internal.getCeDataInode() != 17 || internal.getDeDataInode() != 19 || !internal.dataExists()
                || internal.isInstalled() || !internal.isStopped() || !internal.isNotLaunched() || !internal.isHidden()
                || !internal.isInstantApp() || !internal.isVirtualPreload() || internal.getDistractionFlags() != 3
                || internal.getEnabledState() != 3 || internal.getInstallReason() != 4 || internal.getUninstallReason() != 5
                || internal.getFirstInstallTimeMillis() != 43 || internal.getMinAspectRatio() != 2
                || !internal.getLastDisableAppCaller().equals("caller") || !internal.getHarmfulAppWarning().equals("warning")
                || !internal.getSplashScreenTheme().equals("theme") || !internal.isSuspended() || !internal.isQuarantined()
                || !internal.isComponentEnabled("fixture.Enabled") || !internal.isComponentDisabled("fixture.Disabled")
                || internal.getEnabledComponentsNoCopy().size() != 1 || internal.getDisabledComponentsNoCopy().size() != 1
                || !framework.getEnabledComponents().contains("fixture.Enabled") || !framework.getDisabledComponents().contains("fixture.Disabled")) {
            throw new AssertionError("full user-state interface fields differ");
        }
        try { internal.getEnabledComponentsNoCopy().add("mutated"); throw new AssertionError("unsealed enabled component owner"); }
        catch (IllegalStateException expected) {}
        internal.getEnabledComponents().clear();
        if (!internal.isComponentEnabled("fixture.Enabled")) throw new AssertionError("mutated captured components");
        var map = internal.getSuspendParams();
        if (map.size() != 2) throw new AssertionError("cross-user suspension lost");
        var params = map.get(android.content.pm.UserPackage.of(0, "android"));
        if (!params.isQuarantined() || !params.getDialogInfo().getTitle().equals("title")
                || !params.getDialogInfo().getDialogMessage().equals("message")
                || !params.getDialogInfo().getNeutralButtonText().equals("button")
                || params.getDialogInfo().getNeutralButtonAction() != 1) throw new AssertionError("suspend parameters differ");
        ((int[]) params.getAppExtras().get("values"))[0] = 99;
        if (((int[]) internal.getSuspendParams().get(android.content.pm.UserPackage.of(0, "android")).getAppExtras().get("values"))[0] != 7) {
            throw new AssertionError("mutable suspension extras escaped capture");
        }
        try { map.put(android.content.pm.UserPackage.of(1, "mutated"), params); throw new AssertionError("unsealed suspension map"); }
        catch (IllegalStateException expected) {}
        var perUser = lease.getUserStateReplica(name, false, 10, false);
        if (perUser.getSuspendParams().size() != 1 || perUser.isQuarantined()) throw new AssertionError("per-user duplicate replacement differs");
        if (!internal.getAllOverlayPaths().getOverlayPaths().equals(java.util.List.of("base", "base", "base-apk", "two", "one", "three"))) {
            throw new AssertionError("merged overlays differ");
        }
        internal.getOverlayPaths().getOverlayPaths().clear();
        internal.getSharedLibraryOverlayPaths().get("B").getOverlayPaths().clear();
        if (internal.getAllOverlayPaths().getOverlayPaths().size() != 6) throw new AssertionError("mutated overlay capture");
        var pair = internal.getOverrideLabelIconForComponent(new android.content.ComponentName("fixture", "Activity"));
        if (!pair.first.equals("") || pair.second != 0 || internal.getOverrideLabelIconForComponent(new android.content.ComponentName("fixture", "Absent")) != null) {
            throw new AssertionError("label/icon lookup differs");
        }
        var archive = internal.getArchiveState();
        var activity = archive.getActivityInfos().get(0);
        if (!archive.getInstallerTitle().equals("installer") || archive.getArchiveTimeMillis() != 85
                || !activity.getTitle().equals("archived") || !activity.getOriginalComponentName().getClassName().equals("fixture.Archive")
                || !activity.getIconBitmap().toString().equals("/data/icon") || !activity.getMonochromeIconBitmap().toString().equals("/data/mono")) {
            throw new AssertionError("archive captured timestamp/component differs");
        }
        if (lease.getUserStateReplica(name, false, 0, true).getSuspendParams() != null
                || lease.getUserStateReplica(name, false, 11, true).getSuspendParams() == null
                || lease.getUserStateReplica(name, false, 11, true).getSuspendParams().size() != 0) {
            throw new AssertionError("nullable/empty suspension owners differ");
        }
    }

    private static void runtimeText(java.io.DataOutputStream out, String value) throws Exception {
        byte[] bytes = value == null ? null : value.getBytes(java.nio.charset.StandardCharsets.UTF_8);
        out.writeInt(bytes == null ? -1 : bytes.length); if (bytes != null) out.write(bytes);
    }
    private static void runtimePaths(java.io.DataOutputStream out, android.content.pm.overlay.OverlayPaths paths) throws Exception {
        out.writeBoolean(paths != null); if (paths == null) return;
        for (var list : java.util.List.of(paths.getResourceDirs(), paths.getOverlayPaths())) {
            out.writeInt(list.size()); for (String value : list) runtimeText(out, value);
        }
    }
    private static void runtimeRecord(java.io.DataOutputStream out, com.android.server.pm.pkg.PackageUserStateImpl state, boolean changed) throws Exception {
        out.writeBoolean(changed); runtimePaths(out, state.getOverlayPaths()); runtimePaths(out, state.getAllOverlayPaths());
        var libraries = state.getSharedLibraryOverlayPaths(); out.writeInt(libraries.size());
        for (var entry : libraries.entrySet()) { runtimeText(out, entry.getKey()); runtimePaths(out, entry.getValue()); }
        var value = state.getOverrideLabelIconForComponent(new android.content.ComponentName("fixture", "Activity"));
        out.writeBoolean(value != null);
        if (value != null) { runtimeText(out, value.first); out.writeBoolean(value.second != null); if (value.second != null) out.writeInt(value.second); }
    }
    private static android.content.pm.overlay.OverlayPaths runtimeApk(String... names) {
        var builder = new android.content.pm.overlay.OverlayPaths.Builder();
        for (String name : names) builder.addApkPath(name); return builder.build();
    }
    private static void verifyRuntime(java.io.File file) throws Exception {
        var state = new com.android.server.pm.pkg.PackageUserStateImpl(new com.android.server.utils.WatchableImpl());
        var bytes = new java.io.ByteArrayOutputStream(); var out = new java.io.DataOutputStream(bytes);
        runtimeRecord(out, state, false);
        runtimeRecord(out, state, state.setOverlayPaths(runtimeApk()));
        runtimeRecord(out, state, state.setSharedLibraryOverlayPaths("missing", null));
        var base = new android.content.pm.overlay.OverlayPaths.Builder().addNonApkPath("base").addNonApkPath("base").addApkPath("base-apk").build();
        runtimeRecord(out, state, state.setOverlayPaths(base));
        runtimeRecord(out, state, state.setSharedLibraryOverlayPaths("Aa", runtimeApk("one", "base-apk")));
        runtimeRecord(out, state, state.setSharedLibraryOverlayPaths("B", runtimeApk("two")));
        runtimeRecord(out, state, state.setSharedLibraryOverlayPaths("BB", runtimeApk("three")));
        runtimeRecord(out, state, state.setSharedLibraryOverlayPaths("Aa", runtimeApk("one", "base-apk")));
        var component = new android.content.ComponentName("fixture", "Activity");
        runtimeRecord(out, state, state.overrideLabelAndIcon(component, "", 0));
        runtimeRecord(out, state, state.overrideLabelAndIcon(component, "", 0));
        runtimeRecord(out, state, state.setSharedLibraryOverlayPaths("B", null));
        runtimeRecord(out, state, state.setOverlayPaths(null));
        runtimeRecord(out, state, state.setSharedLibraryOverlayPaths("Aa", null));
        runtimeRecord(out, state, state.setSharedLibraryOverlayPaths("BB", null));
        runtimeRecord(out, state, state.overrideLabelAndIcon(component, null, null));
        if (!java.util.Arrays.equals(bytes.toByteArray(), java.nio.file.Files.readAllBytes(new java.io.File(file.getPath() + ".runtime").toPath()))) {
            throw new AssertionError("native runtime user owner differs from original mutations");
        }
    }

    private static final class PageOwner extends dev.aim.server.IPackageScanSnapshot.Stub {
        private final String name;
        private final byte[] bytes;
        private final byte[] usage;
        private final byte[] seinfo;
        private final byte[] signing;
        byte[] userState, user10, user11, user12, user13, user14, setting, factorySetting, libraries, transientState;
        Integer hiddenApiPolicy;
        int hiddenApiReads;
        int[] userInventory = {0, 10, 11, 12, 13, 14};
        int userReads;
        boolean signingTail;
        byte[] signingOverride;
        int signingReads;
        boolean seinfoTail;
        int seinfoReads;
        boolean usageTail;
        int usageReads;
        boolean fail;
        boolean shortChunk;
        int reads;
        int missingOwner;
        int closes;
        long version = 1;
        PageOwner(String name, byte[] bytes, byte[] usage, byte[] seinfo, byte[] signing) { this.name = name; this.bytes = bytes; this.usage = usage; this.seinfo = seinfo; this.signing = signing; }
        @Override
        public android.os.IInterface queryLocalInterface(String descriptor) { return null; }
        @Override
        public long getVersion() { return version; }
        @Override
        public String[] getPackageNames(boolean disabled) { return disabled ? new String[0] : new String[] {name}; }
        @Override
        public int getCodeLength(String candidate, boolean disabled) { return missingOwner != 1 && !disabled && name.equals(candidate) ? bytes.length : -1; }
        @Override public int getHiddenApiEnforcementPolicy(String candidate, boolean disabled) throws android.os.RemoteException {
            if (fail || hiddenApiPolicy == null) throw new android.os.RemoteException();
            if (disabled || !name.equals(candidate)) throw new IllegalArgumentException("unknown package setting");
            hiddenApiReads++;
            return hiddenApiPolicy;
        }
        @Override public byte[] getTransientState(String candidate, boolean disabled) throws android.os.RemoteException {
            if (fail) throw new android.os.RemoteException();
            return missingOwner != 6 && !disabled && name.equals(candidate) ? transientState : null;
        }
        @Override public int getLibraryStateLength(String candidate) { return missingOwner != 5 && name.equals(candidate) ? libraries.length : -1; }
        @Override public byte[] getLibraryStateChunk(String candidate, int offset, int length) throws android.os.RemoteException {
            if (fail) throw new android.os.RemoteException();
            return java.util.Arrays.copyOfRange(libraries, offset, Math.min(libraries.length, offset + length) - (shortChunk ? 1 : 0));
        }
        @Override public int getSettingLength(String candidate, boolean disabled) { return name.equals(candidate) && (!disabled || factorySetting != null) ? (disabled ? factorySetting.length : setting.length) : -1; }
        @Override public byte[] getSettingChunk(String candidate, boolean disabled, int offset, int length) throws android.os.RemoteException {
            if (fail) throw new android.os.RemoteException();
            byte[] value = disabled ? factorySetting : setting;
            return java.util.Arrays.copyOfRange(value, offset, Math.min(value.length, offset + length) - (shortChunk ? 1 : 0));
        }
        private byte[] userBytes(int userId) {
            return switch (userId) { case 0 -> userState; case 10 -> user10; case 11 -> user11; case 12 -> user12; case 13 -> user13; case 14 -> user14; default -> null; };
        }
        @Override public int[] getUserStateIds(String candidate, boolean disabled) throws android.os.RemoteException {
            if (fail) throw new android.os.RemoteException();
            return !disabled && name.equals(candidate) && userInventory != null ? userInventory.clone() : null;
        }
        @Override public int getUserStateLength(String candidate, boolean disabled, int userId) {
            return !disabled && name.equals(candidate) && (userId == 0 || (userId >= 10 && userId <= 14))
                ? userBytes(userId).length : -1;
        }
        @Override public byte[] getUserStateChunk(String candidate, boolean disabled, int userId, int offset, int length)
                throws android.os.RemoteException {
            if (fail) throw new android.os.RemoteException();
            userReads++;
            byte[] state = userBytes(userId);
            int end = Math.min(state.length, offset + length) - (shortChunk ? 1 : 0);
            return java.util.Arrays.copyOfRange(state, offset, end);
        }
        @Override
        public byte[] getCodeChunk(String name, boolean disabled, int offset, int length) throws android.os.RemoteException {
            if (fail) throw new android.os.RemoteException();
            reads++;
            return java.util.Arrays.copyOfRange(bytes, offset, offset + length - (shortChunk ? 1 : 0));
        }
        @Override
        public void close() { closes++; }
        @Override
        public byte[] getUsage(String candidate) throws android.os.RemoteException {
            if (missingOwner == 3) return null;
            if (fail) throw new android.os.RemoteException();
            usageReads++;
            if (candidate.equals("missing")) return null;
            return usageTail ? java.util.Arrays.copyOf(usage, usage.length + 4) : usage.clone();
        }
        @Override
        public byte[] getSigningState(String candidate, boolean disabled) throws android.os.RemoteException {
            if (missingOwner == 2) return null;
            if (fail) throw new android.os.RemoteException();
            signingReads++;
            if (candidate.equals("missing")) return null;
            byte[] state = signingOverride == null ? signing : signingOverride;
            return signingTail ? java.util.Arrays.copyOf(state, state.length + 4) : state.clone();
        }
        @Override
        public byte[] getSeInfo(String candidate) throws android.os.RemoteException {
            if (missingOwner == 4) return null;
            if (fail) throw new android.os.RemoteException();
            seinfoReads++;
            if (candidate.equals("missing")) return null;
            return seinfoTail ? java.util.Arrays.copyOf(seinfo, seinfo.length + 4) : seinfo.clone();
        }
    }
    private static void skip(org.xmlpull.v1.XmlPullParser parser) throws Exception {
        int depth = parser.getDepth(), event;
        while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {}
    }
    private static void writeMimeString(java.io.DataOutputStream out, String value) throws Exception {
        byte[] bytes = value.getBytes(java.nio.charset.StandardCharsets.UTF_8);
        out.writeInt(bytes.length); out.write(bytes);
    }
    private static void verifyMimeXml(java.io.File file) throws Exception {
        try (var out = new java.io.DataOutputStream(new java.io.FileOutputStream(file.getPath() + ".mime-original"))) {
            for (int i = 0; i < 12; i++) {
                var setting = new com.android.server.pm.PackageSetting("p", null, new java.io.File("/data/p"), 0, 0, new java.util.UUID(1, 1));
                try (var stream = new java.io.FileInputStream(file.getPath() + ".mime-" + i + ".xml")) {
                    var parser = android.util.Xml.resolvePullParser(stream);
                    while (parser.next() != 2) {}
                    int depth = parser.getDepth(), event;
                    while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > depth)) {
                        if (event != 2) continue;
                        if (!parser.getName().equals("mime-group")) { skip(parser); continue; }
                        String name = parser.getAttributeValue(null, "name");
                        if (name == null) { skip(parser); continue; }
                        var types = new android.util.ArraySet<String>();
                        int groupDepth = parser.getDepth();
                        while ((event = parser.next()) != 1 && (event != 3 || parser.getDepth() > groupDepth)) {
                            if (event != 2) continue;
                            if (parser.getName().equals("mime-type")) {
                                String value = parser.getAttributeValue(null, "value"); if (value != null) types.add(value);
                            } else skip(parser);
                        }
                        setting.addMimeTypes(name, types);
                    }
                }
                var groups = setting.getMimeGroups(); out.writeInt(groups.size());
                for (var group : groups.entrySet()) {
                    writeMimeString(out, group.getKey()); out.writeInt(group.getValue().size());
                    for (String value : group.getValue()) writeMimeString(out, value);
                }
            }
        }
    }

    // Pinned Settings.writeMimeGroupLPr loop on original serializers.
    private static void verifyNullableMimeWriter() throws Exception {
        for (boolean binary : new boolean[] {false, true}) for (boolean nullName : new boolean[] {false, true}) {
            var setting = new com.android.server.pm.PackageSetting("p", null, new java.io.File("/data/p"), 0, 0, new java.util.UUID(1, 1));
            var values = new android.util.ArraySet<String>(); values.add(nullName ? "text/plain" : null);
            setting.addMimeTypes(nullName ? null : "types", values);
            var output = new java.io.ByteArrayOutputStream();
            var xml = binary ? android.util.Xml.resolveSerializer(output) : android.util.Xml.newSerializer();
            if (!binary) xml.setOutput(output, "UTF-8");
            try {
                for (var group : setting.getMimeGroups().entrySet()) {
                    xml.startTag(null, "mime-group"); xml.attribute(null, "name", group.getKey());
                    for (String value : group.getValue()) {
                        xml.startTag(null, "mime-type"); xml.attribute(null, "value", value); xml.endTag(null, "mime-type");
                    }
                    xml.endTag(null, "mime-group");
                }
                throw new AssertionError("original MIME writer accepted null name/type");
            } catch (NullPointerException expected) {}
        }
    }

}
