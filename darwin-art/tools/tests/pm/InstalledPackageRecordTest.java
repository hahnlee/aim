package dev.darwinart.runtime.pm;

import java.util.Arrays;

public final class InstalledPackageRecordTest {
    private static void check(boolean value) {
        if (!value) throw new AssertionError();
    }

    private static String record(String body) {
        return "darwin-art-launch-v1\n" + body;
    }

    private static void rejects(String label, String packageName, String body) {
        try {
            InstalledPackageRecord.fromRecord(packageName, record(body));
            throw new AssertionError("accepted " + label);
        } catch (IllegalArgumentException expected) {
            // Expected validation failure.
        }
    }

    public static void main(String[] args) {
        String body = "apk=/installed/base.apk\n"
                + "split=/installed/config.apk\n"
                + "split=/installed/abi.apk\n"
                + "app_id=10042\n";
        InstalledPackageRecord installed =
                InstalledPackageRecord.fromRecord("owner", record(body));
        check(installed.packageName.equals("owner"));
        check(installed.baseApk.equals("/installed/base.apk"));
        check(installed.appId == 10042);
        check(Arrays.equals(installed.splitPaths(), new String[] {
            "/installed/config.apk", "/installed/abi.apk"}));
        check(installed.ownsPrimaryCodePath("/installed/base.apk"));
        check(installed.ownsPrimaryCodePath("/installed/config.apk"));
        check(installed.ownsPrimaryCodePath("/installed/abi.apk"));
        check(!installed.ownsPrimaryCodePath("/installed/foreign.apk"));
        check(installed.dataDirectory().equals("/data/user/0/owner"));

        String[] clone = installed.splitPaths();
        clone[0] = "/foreign.apk";
        check(installed.splitPaths()[0].equals("/installed/config.apk"));

        InstalledPackageRecord missingUid = InstalledPackageRecord.fromRecord(
                "owner", record("apk=/installed/base.apk\n"));
        check(missingUid.appId == -1);

        rejects("duplicate APK", "owner",
                "apk=/installed/base.apk\napk=/installed/other.apk\n");
        rejects("duplicate app ID", "owner",
                "apk=/installed/base.apk\napp_id=10042\napp_id=10043\n");
        rejects("duplicate split", "owner",
                "apk=/installed/base.apk\nsplit=/installed/config.apk\n"
                        + "split=/installed/config.apk\n");
        rejects("base listed as split", "owner",
                "apk=/installed/base.apk\nsplit=/installed/base.apk\n");
        rejects("relative APK", "owner", "apk=relative.apk\n");
        rejects("relative split", "owner",
                "apk=/installed/base.apk\nsplit=relative.apk\n");
        rejects("negative app ID", "owner",
                "apk=/installed/base.apk\napp_id=-1\n");
        for (String uid : new String[]{"0", "1000", "9999", "20000", "110042"}) {
            rejects("non-registry app ID", "owner", "apk=/installed/base.apk\napp_id=" + uid + "\n");
        }
        for (String uid : new String[]{"10000", "19999"}) {
            check(InstalledPackageRecord.fromRecord("owner",
                    record("apk=/installed/base.apk\napp_id=" + uid + "\n")).appId
                    == Integer.parseInt(uid));
        }
        rejects("invalid package", "bad/name", "apk=/installed/base.apk\n");
        try {
            InstalledPackageRecord.fromRecord("owner", "apk=/installed/base.apk\n");
            throw new AssertionError("accepted missing record header");
        } catch (IllegalArgumentException expected) {
            // Expected validation failure.
        }

        // The runner mounts the host package store at PACKAGE_ROOT.
        String root = System.getenv("DARWIN_ART_ANDROID_PACKAGE_ROOT");
        check("/profile/packages".equals(root));
        check(InstalledPackageRecord.guestCodePath("/profile/packages/owner/1/base.apk")
                .equals("/data/app/owner/1/base.apk"));
        check(InstalledPackageRecord.guestCodePath("/profile/packagesX/base.apk") == null);
        check(InstalledPackageRecord.guestCodePath("/elsewhere/base.apk") == null);
        check(InstalledPackageRecord.hostCodePath("/data/app/owner/1/split.apk")
                .equals("/profile/packages/owner/1/split.apk"));
        check(InstalledPackageRecord.hostCodePath("/data/user/0/owner")
                .equals("/data/user/0/owner"));
        System.out.println("InstalledPackageRecord validation PASS (typed registry fixture)");
    }
}
