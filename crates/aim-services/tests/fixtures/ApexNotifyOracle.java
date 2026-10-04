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
