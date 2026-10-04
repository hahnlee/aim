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
        System.out.println("APEX_NOTIFY " + count);
    }
    private ApexNotifyOracle() {}
}
