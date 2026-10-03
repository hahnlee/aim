package com.android.server.pm;

import android.util.AtomicFile;
import java.io.File;
import java.nio.file.Files;
import java.util.Arrays;
import java.util.Map;
import java.util.UUID;

/** Original PackageUsage reads/writes only this oracle's disposable file. */
public final class PackageUsageOracle {
    private static final class Usage extends PackageUsage {
        private final File file;
        Usage(File file) { this.file = file; }
        @Override protected AtomicFile getFile() { return new AtomicFile(file); }
    }
    private static void check(boolean value) {
        if (!value) throw new AssertionError("package usage contract changed");
    }
    private static PackageSetting setting(String name) {
        return new PackageSetting(name, null, new File("/data/app/" + name),
                0, 0, new UUID(1, 1));
    }
    public static void main(String[] args) throws Exception {
        File dir = new File(args[0]);
        var a = setting("a"); var b = setting("b");
        var packages = Map.of("a", a, "b", b);
        var usage = new Usage(new File(dir, "usage.list"));
        Files.copy(new File(dir, "native.list").toPath(), new File(dir, "usage.list").toPath());
        usage.readInternal(packages);
        check(Arrays.equals(a.getPkgState().getLastPackageUsageTimeInMills(),
                new long[]{9,10,44,4,5,6,7,8}));
        check(a.getPkgState().getLatestPackageUseTimeInMills() == 44);
        check(a.getPkgState().getLatestForegroundPackageUseTimeInMills() == 44);
        check(Arrays.equals(b.getPkgState().getLastPackageUsageTimeInMills(), new long[8]));
        a.getPkgState().setLastPackageUsageTimeInMills(-1, 99);
        a.getPkgState().setLastPackageUsageTimeInMills(8, 99);
        check(a.getPkgState().getLatestPackageUseTimeInMills() == 44);
        a.getPkgState().setLastPackageUsageTimeInMills(7, 55);
        usage.writeInternal(packages);
        check(Files.exists(new File(dir, "usage.list").toPath()));
        var legacy = new Usage(new File(dir, "legacy.list"));
        legacy.readInternal(packages);
        check(Arrays.equals(a.getPkgState().getLastPackageUsageTimeInMills(),
                new long[]{7,7,7,7,7,7,7,7}));
        check(Arrays.equals(b.getPkgState().getLastPackageUsageTimeInMills(),
                new long[]{-4,-4,-4,-4,-4,-4,-4,-4}));
        check(b.getPkgState().getLatestPackageUseTimeInMills() == 0);
        var partial = new Usage(new File(dir, "partial.list"));
        partial.readInternal(packages);
        check(Arrays.equals(a.getPkgState().getLastPackageUsageTimeInMills(),
                new long[]{9,10,7,7,7,7,7,7}));
        check(partial.isHistoricalPackageUsageAvailable());
        var missing = new Usage(new File(dir, "missing.list"));
        missing.readInternal(packages);
        check(!missing.isHistoricalPackageUsageAvailable());
        System.out.println("USAGE native original versions reasons prefix historical");
    }
}
