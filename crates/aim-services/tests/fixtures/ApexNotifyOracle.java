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
        System.out.println("APEX_NOTIFY " + count);
    }
    private ApexNotifyOracle() {}
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
