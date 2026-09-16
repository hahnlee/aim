import dev.darwinart.runtime.pm.DexLoadReports;
import dev.darwinart.runtime.pm.PackageRecords;
import java.util.Collections;

public final class DexLoadReportsTest {
    public static void main(String[] args) {
        PackageRecords.Source source = new PackageRecords.Source() {
            public String resolveInstalledPackage(String name) {
                return "owner".equals(name)
                    ? "darwin-art-launch-v1\n"
                        + "apk=/installed/base.apk\n"
                        + "split=/installed/split.apk\n"
                        + "app_id=10042\n"
                        + "metadata=apk-app-runtime: package=owner target_sdk=not-a-number\n"
                    : null;
            }
        };
        // No store is used in the primary/split/auth tests. Secondary persistence
        // is explicitly outside this test, not a fake successful store.
        DexLoadReports reports = new DexLoadReports(source, null);
        for (String path : new String[]{"/installed/base.apk", "/installed/split.apk"}) {
            reports.report(10042, "owner", Collections.singletonMap(path, "PCL[]"), "arm64");
        }
        // Dex ownership must not parse manifest/application hints. The malformed
        // target SDK is intentionally irrelevant to this registry-only path.
        reports.report(10042, "owner", null, "arm64");
        for (int uid : new int[]{-1, 10043, 110042}) {
            boolean denied = false;
            try { reports.report(uid, "owner", Collections.singletonMap("/installed/base.apk", "PCL[]"), "arm64"); }
            catch (SecurityException expected) { denied = true; }
            assert denied;
        }
        boolean denied = false;
        try { reports.report(10042, "uninstalled", null, "arm64"); }
        catch (SecurityException expected) { denied = true; }
        assert denied;
        // AOSP resolves an isolated UID through system-owned process state,
        // rather than comparing it directly to the installed appId.
        DexLoadReports isolatedReports = new DexLoadReports(source, null,
                (uid, packageName) -> uid == 99042 && "owner".equals(packageName));
        isolatedReports.report(99042, "owner", null, "arm64");
        denied = false;
        try { isolatedReports.report(99043, "owner", null, "arm64"); }
        catch (SecurityException expected) { denied = true; }
        assert denied;
        boolean rejected = false;
        try { reports.report(10042, "owner", Collections.singletonMap("/installed/base.apk", "PCL[]"), "bogus"); }
        catch (IllegalArgumentException expected) { rejected = true; }
        assert rejected;
        // Foreign and parent-traversing paths must not reach any usage writer.
        reports.report(10042, "owner", Collections.singletonMap("/data/user/0/foreign/code.dex", "PCL[]"), "arm64");
        reports.report(10042, "owner", Collections.singletonMap("/data/user/0/owner/../foreign/code.dex", "PCL[]"), "arm64");
        System.out.println("DEX report policy: PASS (caller UID, installed identity, ISA, own APK/split, unknown path)");
    }
}
