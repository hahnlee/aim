package com.android.server.pm.dex;
import java.io.*;

public final class PackageDexUsageTest {
    public static void main(String[] args) throws Exception {
        PackageDexUsage usage = new PackageDexUsage();
        assert usage.record("owner", "/data/app/owner/base.apk", 0, "arm64", true, "loader", "PCL[]", false);
        assert usage.getPackageUseInfo("owner").isUsedByOtherApps("/data/app/owner/base.apk");
        assert usage.record("owner", "/data/user/0/owner/code.dex", 0, "arm64", false, "owner", "PCL[]", false);
        assert !usage.record("owner", "/data/user/0/owner/code.dex", 0, "arm64", false, "owner", "PCL[]", false);
        assert usage.record("owner", "/data/user/0/owner/code.dex", 0, "arm64", false, "loader", "PCL[/other]", false);
        PackageDexUsage.DexUseInfo info = usage.getPackageUseInfo("owner").getDexUseInfoMap().get("/data/user/0/owner/code.dex");
        assert info.isUsedByOtherApps();
        assert info.getClassLoaderContext().equals(PackageDexUsage.VARIABLE_CLASS_LOADER_CONTEXT);
        assert info.getLoadingPackages().contains("loader");
        boolean rejected = false;
        try { usage.record("owner", "/data/user/0/owner/code.dex", 10, "arm64", false, "owner", "PCL[]", false); }
        catch (IllegalArgumentException expected) { rejected = true; }
        assert rejected;
        rejected = false;
        try { usage.record("owner", "/invalid.dex", 0, "invalid-isa", false, "owner", "PCL[]", false); }
        catch (IllegalArgumentException expected) { rejected = true; }
        assert rejected;
        for (int i = 0; i < 200; i++) usage.record("bounded", "/data/user/0/bounded/" + i + ".dex", 0, "arm64", false, "bounded", "PCL[]", false);
        assert usage.getPackageUseInfo("bounded").getDexUseInfoMap().size() == 100;
        StringWriter output = new StringWriter();
        usage.write(output);
        assert output.toString().startsWith("PACKAGE_MANAGER__PACKAGE_DEX_USAGE__2\n");
        PackageDexUsage restored = new PackageDexUsage();
        restored.read(new StringReader(output.toString()));
        assert restored.getPackageUseInfo("owner").isUsedByOtherApps("/data/app/owner/base.apk");
        assert restored.getPackageUseInfo("owner").getDexUseInfoMap().get("/data/user/0/owner/code.dex").getClassLoaderContext().equals(info.getClassLoaderContext());
        assert restored.getPackageUseInfo("bounded").getDexUseInfoMap().size() == 100;
        System.out.println("Original PackageDexUsage: PASS (merge, isolation, bound, version-2 round trip)");
    }
}
