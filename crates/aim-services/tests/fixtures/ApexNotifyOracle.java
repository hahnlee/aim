package com.android.server.pm;

public final class ApexNotifyOracle {
    public static void main(String[] args) throws Exception {
        var directory = new java.io.File(args[0]);
        byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "apex-notify.input").toPath());
        var owner = new ApexManager.ApexManagerImpl();
        ApexBootFeed.notifyScanResults(owner, bytes);
        var in = android.os.Parcel.obtain();
        int count;
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0); count = in.readInt();
            for (int i = 0; i < count; i++) {
                String module = in.readString(); in.readString(); in.readString(); in.readLong();
                in.readBoolean(); boolean active = in.readBoolean(); in.readBoolean();
                byte[] cache = in.createByteArray(); int pastCount = in.readInt();
                for (int j = 0; j < pastCount; j++) { in.createByteArray(); in.readInt(); }
                var pkg = com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(cache);
                String name = ((com.android.internal.pm.parsing.pkg.PackageImpl) pkg).getPackageName();
                if (!java.util.Objects.equals(module, owner.getApexModuleNameForPackageName(name))) throw new AssertionError("module owner differs");
                if (active && !name.equals(owner.getActivePackageNameForApexModuleName(module))) throw new AssertionError("active owner differs");
            }
        } finally { in.recycle(); }
        byte[] trailing = java.util.Arrays.copyOf(bytes, bytes.length + 4);
        try { ApexBootFeed.notifyScanResults(owner, trailing); throw new AssertionError("trailing payload accepted"); }
        catch (IllegalArgumentException expected) {}
        byte[] unaligned = java.util.Arrays.copyOf(bytes, bytes.length - 1);
        try { ApexBootFeed.notifyScanResults(owner, unaligned); throw new AssertionError("unaligned payload accepted"); }
        catch (IllegalArgumentException expected) {}
        try { ApexBootFeed.notifyScanResults(owner, new byte[0]); throw new AssertionError("missing count accepted"); }
        catch (IllegalArgumentException expected) {}
        verifySharedIds(directory);
        verifySharedApex(directory);
        verifyChangedGroup(directory);
        verifyNegativeUidConversion(directory);
        verifyConversionEligibility(directory);
        verifyLegacyConstructors(directory);
        verifyRetainedRename(directory);
        verifyOriginalAdoptionSlot(directory);
        verifyDisabledInheritance(directory);
        System.out.println("APEX_NOTIFY " + count);
        // The test-only Settings constructor starts BackgroundThread.
        System.exit(0);
    }
    private ApexNotifyOracle() {}
    private static void verifySharedApex(java.io.File directory) throws Exception {
        byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "shared-apex.input").toPath());
        var in = android.os.Parcel.obtain();
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            var pkg = (com.android.internal.pm.parsing.pkg.PackageImpl)
                com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(in.createByteArray());
            int flags = in.readInt(), privateFlags = in.readInt();
            String label = in.readString(); int sdk = in.readInt(), registeredAppId = in.readInt();
            if (in.dataAvail() != 0 || pkg.getUid() != -1 || !pkg.isApex()) throw new AssertionError("shared APEX code differs");
            var group = new SharedUserSetting("aim.fixture.apex", 0, 0); group.mAppId = 10000;
            var setting = new PackageSetting(pkg.getPackageName(), null, new java.io.File(pkg.getPath()),
                flags, privateFlags, new java.util.UUID(1, 1));
            setting.setAppId(10000); setting.setSharedUserAppId(10000);
            String originalLabel = SELinuxMMAC.getSeInfo((com.android.server.pm.pkg.PackageState)setting, pkg, (privateFlags & 8) != 0, pkg.getTargetSdkVersion());
            if (!java.util.Objects.equals(label, originalLabel)) throw new AssertionError("shared scan seInfo differs: " + label + " != " + originalLabel);
            setting.setAppId(-1); setting.setPkg((com.android.server.pm.pkg.AndroidPackage)(Object)pkg); group.addPackage(setting);
            if (group.getSeInfoTargetSdkVersion() != sdk || sdk != pkg.getTargetSdkVersion()) throw new AssertionError("first committed shared SDK differs");
            var settings = new Settings(java.util.Map.of());
            var registeredGroup = settings.addSharedUserLPw("aim.fixture.apex", 10000, 0, 0);
            settings.addPackageSettingLPw(setting, registeredGroup);
            if (setting.getAppId() != registeredAppId || registeredAppId != 10000 || ((com.android.server.pm.pkg.PackageState)setting).getSharedUserAppId() != 10000 || pkg.getUid() != -1) throw new AssertionError("registered shared APEX IDs differ");
        } finally { in.recycle(); }
    }
    private static void verifyNegativeUidConversion(java.io.File directory) throws Exception {
        var settings = new Settings(java.util.Map.of());
        var group = settings.addSharedUserLPw("aim.fixture.leaving", 10000, 0, 0);
        var setting = new PackageSetting("fixture", null, new java.io.File("/system/apex/fixture.apex"),
            1, 0, new java.util.UUID(1, 1));
        setting.setAppId(-1); setting.setSharedUserAppId(10000);
        group.addPackage(setting);
        var in = android.os.Parcel.obtain();
        byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "apex-conversion.input").toPath());
        int nativeAppId, nativeSharedId; boolean nativeSlot;
        try {
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            nativeAppId = in.readInt(); nativeSharedId = in.readInt(); nativeSlot = in.readBoolean();
            if (in.dataAvail() != 0) throw new AssertionError("APEX conversion tail");
        } finally { in.recycle(); }
        int[] users = {10, 0, 11};
        byte[] legacy = java.nio.file.Files.readAllBytes(new java.io.File(directory, "legacy-permissions.original").toPath());
        setting.getLegacyPermissionState().copyFrom(dev.aim.server.PackageLegacyPermissions.restore(10042, users, legacy));
        setting.setInstallPermissionsFixed(true);
        settings.convertSharedUserSettingsLPw(group);
        if (setting.getAppId() != nativeAppId || nativeAppId != -1
                || ((com.android.server.pm.pkg.PackageState)setting).getSharedUserAppId() != nativeSharedId || nativeSharedId != -1
                || !nativeSlot || settings.getSettingLPr(10000) != setting)
            throw new AssertionError("negative APEX shared UID conversion differs");
        byte[] nativeLegacy = java.nio.file.Files.readAllBytes(new java.io.File(directory, "apex-conversion-legacy.input").toPath());
        if (!setting.isInstallPermissionsFixed() || !java.util.Arrays.equals(nativeLegacy,
                dev.aim.server.PackageLegacyPermissions.capture(-1, users, setting.getLegacyPermissionState())))
            throw new AssertionError("converted APEX legacy/fixed state differs");
        settings.addPackageSettingLPw(setting, null);
        if (setting.getAppId() != -1 || settings.getSettingLPr(10000) != setting)
            throw new AssertionError("negative APEX converted slot changed at registration");
    }
    private static void verifyConversionEligibility(java.io.File directory) throws Exception {
        var in = android.os.Parcel.obtain();
        var original = android.os.Parcel.obtain();
        try {
            byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "shared-apex.input").toPath());
            original.unmarshall(bytes, 0, bytes.length); original.setDataPosition(0);
            var retainedCode = (com.android.internal.pm.parsing.pkg.PackageImpl)
                com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(original.createByteArray());
            bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "apex-conversion-cases.input").toPath());
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            var leavingCode = (com.android.internal.pm.parsing.pkg.PackageImpl)
                com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(in.createByteArray());
            if (in.readInt() != 4) throw new AssertionError("conversion case count");
            for (int i = 0; i < 4; i++) {
                int mode = in.readInt(); boolean converts = in.readBoolean();
                int appId = in.readInt(), sharedId = in.readInt();
                var settings = new Settings(java.util.Map.of());
                var group = settings.addSharedUserLPw("aim.fixture.leaving", 10000, 0, 0);
                var active = new PackageSetting("fixture", null, new java.io.File("/system/apex/fixture.apex"), 1, 0, new java.util.UUID(1, 1));
                active.setPkg((com.android.server.pm.pkg.AndroidPackage)(Object)(mode == 1 ? retainedCode : leavingCode));
                settings.addPackageSettingLPw(active, group);
                if ((mode == 1 || mode == 2) && !settings.disableSystemPackageLPw("fixture", true))
                    throw new AssertionError("conversion factory disable failed");
                if (mode == 3) {
                    var other = new PackageSetting("fixture.other", null, new java.io.File("/system/app/other.apk"), 0, 0, new java.util.UUID(1, 2));
                    settings.addPackageSettingLPw(other, group);
                }
                active.setAppId(-1);
                active.setPkg((com.android.server.pm.pkg.AndroidPackage)(Object)leavingCode);
                if (group.isSingleUser() != converts) throw new AssertionError("conversion eligibility differs");
                if (converts) settings.convertSharedUserSettingsLPw(group);
                settings.addPackageSettingLPw(active, converts ? null : group);
                if (active.getAppId() != appId || ((com.android.server.pm.pkg.PackageState)active).getSharedUserAppId() != sharedId
                        || settings.getSettingLPr(10000) != (converts ? active : group))
                    throw new AssertionError("conversion active ownership differs");
                if ((mode == 1 || mode == 2)) {
                    var factory = settings.getDisabledSystemPkgLPr("fixture");
                    if (factory.getAppId() != in.readInt() || ((com.android.server.pm.pkg.PackageState)factory).getSharedUserAppId() != in.readInt())
                        throw new AssertionError("conversion factory ownership differs");
                }
            }
            if (in.dataAvail() != 0) throw new AssertionError("conversion cases tail");
        } finally { in.recycle(); original.recycle(); }
    }
    private static void verifyOriginalAdoptionSlot(java.io.File directory) throws Exception {
        var settings = new Settings(java.util.Map.of());
        var old = new PackageSetting("original.fixture", null,
            new java.io.File("/system/apex/original.fixture.apex"), 1, 0, new java.util.UUID(1, 1));
        old.setAppId(10000); old.setInstallPermissionsFixed(true);
        int[] users = {10, 0, 11};
        byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "legacy-permissions.original").toPath());
        old.getLegacyPermissionState().copyFrom(dev.aim.server.PackageLegacyPermissions.restore(10042, users, bytes));
        settings.registerAppIdLPw(old, false); settings.addPackageSettingLPw(old, null);
        var adopted = Settings.createNewSetting("incoming.fixture", old, null, null, null,
            new java.io.File("/system/apex/incoming.fixture.apex"), null, null, null, 2, 1, 0,
            null, true, false, false, false, null, null, null, null, null, null, null,
            new java.util.UUID(1, 2), 36, null);
        adopted.setAppId(-1);
        settings.addRenamedPackageLPw("incoming.fixture", old.getPackageName());
        settings.addPackageSettingLPw(adopted, null);
        if (settings.getSettingLPr(10000) != old || old.getAppId() != 10000
                || adopted.getAppId() != -1 || adopted == old
                || !old.getPackageName().equals(adopted.getPackageName())
                || !"incoming.fixture".equals(adopted.getRealName())
                || old.getLegacyPermissionState() == adopted.getLegacyPermissionState()
                || !adopted.isInstallPermissionsFixed())
            throw new AssertionError("original APEX adoption slot/copy differs");
        var out = android.os.Parcel.obtain();
        try {
            out.writeInt(old.getAppId()); out.writeInt(adopted.getAppId());
            out.writeByteArray(dev.aim.server.PackageLegacyPermissions.capture(10000, users, old.getLegacyPermissionState()));
            out.writeByteArray(dev.aim.server.PackageLegacyPermissions.capture(-1, users, adopted.getLegacyPermissionState()));
            java.nio.file.Files.write(new java.io.File(directory, "apex-original-adoption.original").toPath(), out.marshall());
        } finally { out.recycle(); }
    }
    private static void verifyRetainedRename(java.io.File directory) throws Exception {
        var in = android.os.Parcel.obtain();
        try {
            byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "apex-retained-rename.input").toPath());
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            var pkg = (com.android.internal.pm.parsing.pkg.PackageImpl)
                com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(in.createByteArray());
            String oldName = in.readString(), realName = in.readString(), path = in.readString();
            int flags = in.readInt(), privateFlags = in.readInt(), target = in.readInt();
            byte[] nativeLegacy = in.createByteArray(); boolean fixed = in.readBoolean();
            if (in.dataAvail() != 0 || !oldName.equals(pkg.getPackageName())
                    || !realName.equals(pkg.getManifestPackageName()) || pkg.getUid() != -1)
                throw new AssertionError("retained renamed APEX code differs");
            var settings = new Settings(java.util.Map.of());
            settings.addRenamedPackageLPw(realName, oldName);
            if (!oldName.equals(settings.getRenamedPackageLPr(realName)))
                throw new AssertionError("original rename owner differs");
            var retained = new PackageSetting(oldName, realName, new java.io.File(path),
                flags, privateFlags, new java.util.UUID(0, 0));
            retained.setAppId(-1); retained.setInstallPermissionsFixed(true);
            int[] users = {10, 0, 11};
            bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "legacy-permissions.original").toPath());
            retained.getLegacyPermissionState().copyFrom(dev.aim.server.PackageLegacyPermissions.restore(10042, users, bytes));
            Settings.updatePackageSetting(retained, null, null, null, new java.io.File(path),
                null, null, null, flags, privateFlags, null, null, null, null, null, null,
                java.util.Set.of(), new java.util.UUID(0, 1), target, null, false);
            if (!oldName.equals(retained.getPackageName()) || !realName.equals(retained.getRealName())
                    || retained.getAppId() != -1 || retained.isInstallPermissionsFixed() != fixed
                    || !java.util.Arrays.equals(nativeLegacy,
                        dev.aim.server.PackageLegacyPermissions.capture(-1, users, retained.getLegacyPermissionState())))
                throw new AssertionError("retained renamed APEX setting differs");
            pkg.setPackageName(realName); pkg.setPackageName(oldName);
            if (!oldName.equals(pkg.getPackageName()) || !realName.equals(pkg.getManifestPackageName()))
                throw new AssertionError("original parsed rename differs");
        } finally { in.recycle(); }
    }
    private static void verifyLegacyConstructors(java.io.File directory) throws Exception {
        int[] users = {10, 0, 11};
        byte[] populated = java.nio.file.Files.readAllBytes(new java.io.File(directory, "legacy-permissions.original").toPath());
        var legacy = dev.aim.server.PackageLegacyPermissions.restore(10042, users, populated);
        var in = android.os.Parcel.obtain();
        try {
            byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "apex-constructor-legacy.input").toPath());
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            if (in.readInt() != 2) throw new AssertionError("constructor case count");
            for (int i = 0; i < 2; i++) {
                boolean disabled = in.readBoolean();
                var settings = new Settings(java.util.Map.of());
                var oldGroup = settings.getSharedUserLPw("old", 0, 0, true);
                oldGroup.getLegacyPermissionState().copyFrom(legacy);
                var old = new PackageSetting("fixture", null, new java.io.File("/system/apex/fixture.apex"), 1, 0, new java.util.UUID(1, 1));
                old.setPkg((com.android.server.pm.pkg.AndroidPackage)(Object)com.android.internal.pm.parsing.pkg.PackageImpl.forTesting("fixture"));
                old.getLegacyPermissionState().copyFrom(legacy); old.setInstallPermissionsFixed(true);
                settings.addPackageSettingLPw(old, oldGroup);
                if (disabled && !settings.disableSystemPackageLPw("fixture", true)) throw new AssertionError("constructor factory disable failed");
                var group = settings.getSharedUserLPw("new", 0, 0, true);
                if (oldGroup.mAppId != 10000 || group.mAppId != 10001) throw new AssertionError("constructor allocation differs");
                var replacement = Settings.createNewSetting("fixture", null,
                    disabled ? settings.getDisabledSystemPkgLPr("fixture") : null, null, group,
                    new java.io.File("/data/apex/active/fixture.apex"), null, null, null, 1, 1, 0,
                    null, true, false, false, false, null, null, null, null, null, null, null,
                    new java.util.UUID(1, 2), 36, null);
                replacement.setAppId(-1);
                oldGroup.removePackage(old); settings.checkAndPruneSharedUserLPw(oldGroup, false);
                settings.addPackageSettingLPw(replacement, group);
                if (!java.util.Arrays.equals(in.createByteArray(), dev.aim.server.PackageLegacyPermissions.capture(10001, users, replacement.getLegacyPermissionState()))
                        || in.readBoolean() != replacement.isInstallPermissionsFixed()
                        || !java.util.Arrays.equals(in.createByteArray(), dev.aim.server.PackageLegacyPermissions.capture(10001, users, group.getLegacyPermissionState())))
                    throw new AssertionError("fresh constructor legacy differs");
                if (disabled) {
                    var factory = settings.getDisabledSystemPkgLPr("fixture");
                    if (in.readBoolean() != factory.isInstallPermissionsFixed()
                            || !java.util.Arrays.equals(in.createByteArray(), dev.aim.server.PackageLegacyPermissions.capture(10000, users, factory.getLegacyPermissionState()))
                            || !java.util.Arrays.equals(in.createByteArray(), dev.aim.server.PackageLegacyPermissions.capture(10000, users, oldGroup.getLegacyPermissionState())))
                        throw new AssertionError("retained factory/group legacy differs");
                } else if (settings.getSettingLPr(10000) != null) throw new AssertionError("old group not pruned");
            }
            if (in.dataAvail() != 0) throw new AssertionError("constructor legacy tail");
            var rejected = new Settings(java.util.Map.of());
            rejected.getSharedUserLPw("old", 0, 0, true);
            var allocated = rejected.getSharedUserLPw("new", 0, 0, true);
            bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "apex-rejected-group-legacy.input").toPath());
            if (!java.util.Arrays.equals(bytes, dev.aim.server.PackageLegacyPermissions.capture(10001, users, allocated.getLegacyPermissionState())))
                throw new AssertionError("rejected allocation constructor differs");
        } finally { in.recycle(); }
    }
    private static void verifyChangedGroup(java.io.File directory) throws Exception {
        var in = android.os.Parcel.obtain();
        try {
            byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "apex-group-change.input").toPath());
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            if (in.readInt() != 3) throw new AssertionError("group transition count differs");
            for (int i = 0; i < 3; i++) {
                boolean disabled = in.readBoolean(); int appId = in.readInt(); boolean keepsOld = in.readBoolean();
                var settings = new Settings(java.util.Map.of());
                var oldGroup = settings.addSharedUserLPw("old", 10000, 0, 0);
                var newGroup = appId == -1 ? null : settings.addSharedUserLPw("new", 10001, 0, 0);
                var code = com.android.internal.pm.parsing.pkg.PackageImpl.forTesting("fixture");
                var old = new PackageSetting("fixture", null, new java.io.File("/system/apex/fixture.apex"), 1, 0, new java.util.UUID(1, 1));
                old.setPkg((com.android.server.pm.pkg.AndroidPackage)(Object)code);
                settings.addPackageSettingLPw(old, oldGroup);
                if (disabled && !settings.disableSystemPackageLPw("fixture", true)) throw new AssertionError("factory did not disable");
                oldGroup.removePackage(old);
                settings.checkAndPruneSharedUserLPw(oldGroup, false);
                var replacement = new PackageSetting("fixture", null, new java.io.File("/system/apex/fixture.apex"), 1, 0, new java.util.UUID(1, 2));
                replacement.setAppId(-1); if (newGroup != null) replacement.setSharedUserAppId(10001);
                replacement.setPkg((com.android.server.pm.pkg.AndroidPackage)(Object)code);
                settings.addPackageSettingLPw(replacement, newGroup);
                if (replacement.getAppId() != appId || (settings.getSettingLPr(10000) != null) != keepsOld)
                    throw new AssertionError("changed group registration/pruning differs");
            }
            if (in.dataAvail() != 0) throw new AssertionError("group transition tail");
        } finally { in.recycle(); }
    }
    private static void verifyDisabledInheritance(java.io.File directory) throws Exception {
        var in = android.os.Parcel.obtain();
        try {
            byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "shared-apex.input").toPath());
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            var code = (com.android.internal.pm.parsing.pkg.PackageImpl)com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(in.createByteArray());
            int flags = in.readInt(), privateFlags = in.readInt();
            var factory = new PackageSetting(code.getPackageName(), null, new java.io.File(code.getPath()), flags, privateFlags, new java.util.UUID(1, 1));
            factory.setAppId(10000); factory.setSharedUserAppId(10000);
            factory.setSigningDetails(code.getSigningDetails()); factory.setInstallPermissionsFixed(true);
            int[] users = {10, 0, 11};
            byte[] legacy = java.nio.file.Files.readAllBytes(new java.io.File(directory, "legacy-permissions.original").toPath());
            factory.getLegacyPermissionState().copyFrom(dev.aim.server.PackageLegacyPermissions.restore(10042, users, legacy));
            var factoryUser = factory.getOrCreateUserState(0);
            var enabled = new android.util.ArraySet<String>(); enabled.add("enabled.fixture");
            var disabled = new android.util.ArraySet<String>(); disabled.add("disabled.fixture");
            factoryUser.setEnabledComponents(enabled); factoryUser.setDisabledComponents(disabled); factoryUser.setHidden(true); factoryUser.setInstalled(false);
            var replacement = Settings.createNewSetting(code.getPackageName(), null, factory, null, null,
                new java.io.File("/data/apex/active/shared-replacement.apex"), null, null, null, 1,
                flags, privateFlags, null, true, false, false, false, null,
                null, null, null, null, null, null, new java.util.UUID(1, 2), code.getTargetSdkVersion(), null);
            replacement.setAppId(-1);
            byte[] nativeBytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "disabled-apex-legacy.input").toPath());
            if (!java.util.Arrays.equals(nativeBytes, dev.aim.server.PackageLegacyPermissions.capture(-1, users, replacement.getLegacyPermissionState())))
                throw new AssertionError("disabled legacy inheritance differs");
            if (replacement.isInstallPermissionsFixed() || !factory.isInstallPermissionsFixed()
                    || !replacement.getSigningDetails().equals(factory.getSigningDetails())) throw new AssertionError("factory signatures/fixed bit differs");
            // Settings' enumerated-user loop uses these original copy setters.
            replacement.setEnabledComponentsCopy(factoryUser.getEnabledComponentsNoCopy(), 0);
            replacement.setDisabledComponentsCopy(factoryUser.getDisabledComponentsNoCopy(), 0);
            var user = (com.android.server.pm.pkg.PackageUserStateInternal)(Object)replacement.getOrCreateUserState(0);
            if (user.isHidden() || !user.isInstalled() || !user.getEnabledComponents().equals(enabled) || !user.getDisabledComponents().equals(disabled))
                throw new AssertionError("disabled component inheritance differs");
            user.getEnabledComponents().add("late.fixture");
            if (((com.android.server.pm.pkg.PackageUserStateInternal)(Object)factoryUser).getEnabledComponents().contains("late.fixture")) throw new AssertionError("factory aliases replacement user");
        } finally { in.recycle(); }
    }
    private static void verifySharedIds(java.io.File directory) throws Exception {
        var in = android.os.Parcel.obtain();
        try {
            byte[] bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "shared-id.setting").toPath());
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            var data = dev.aim.server.PackageSettingData.read(in);
            if (in.dataAvail() != 0 || data.appId != 10123 || !data.sharedUser || data.sharedUserAppId != 1000)
                throw new AssertionError("distinct scalar IDs differ");
            var setting = new PackageSetting(data.getPackageName(), data.realName,
                new java.io.File(data.path), data.flags, data.privateFlags, new java.util.UUID(1, 1));
            setting.setAppId(data.appId); setting.setSharedUserAppId(data.sharedUserAppId);
            bytes = java.nio.file.Files.readAllBytes(new java.io.File(directory, "shared-id.signing").toPath());
            in.unmarshall(bytes, 0, bytes.length); in.setDataPosition(0);
            var signing = dev.aim.server.PackageSigningState.CREATOR.createFromParcel(in);
            if (in.dataAvail() != 0 || signing.getAppId() != 10123 || signing.getSharedAppId() != 1000)
                throw new AssertionError("distinct signing IDs differ");
            dev.aim.server.PackageObjects.restoreSavedSigning(setting, signing, 1, false);
            setting.setAppId(-1);
            if (setting.getAppId() != -1 || !setting.hasSharedUser()
                    || ((com.android.server.pm.pkg.PackageState)setting).getSharedUserAppId() != 1000)
                throw new AssertionError("APEX app ID discarded shared relationship");
        } finally { in.recycle(); }
    }
}
