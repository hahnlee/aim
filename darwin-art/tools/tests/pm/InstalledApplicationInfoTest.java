import dev.darwinart.runtime.pm.InstalledApplicationInfo;
import android.content.pm.ApplicationInfo;

public final class InstalledApplicationInfoTest {
    private static void check(boolean value) {
        if (!value) throw new AssertionError();
    }

    private static String record(String apk, String splits, String appId, String metadata) {
        return "darwin-art-launch-v1\n"
                + "apk=" + apk + "\n"
                + (splits == null ? "" : splits)
                + (appId == null ? "" : "app_id=" + appId + "\n")
                + (metadata == null ? "" : "metadata=" + metadata + "\n");
    }

    public static void main(String[] args) {
        String record = record("/packages/other/base.apk",
                "split=/packages/other/config.apk\nsplit=/packages/other/abi.apk\n", "10042",
                "apk-app-runtime: package=other application=other.App "
                        + "target_sdk=36 has_code=0 debuggable=1 supports_rtl=1 "
                        + "split_names=config.foo,config.arm64_v8a");
        ApplicationInfo info = InstalledApplicationInfo.fromRecord("other", record);
        check(info.packageName.equals("other") && info.className.equals("other.App"));
        check(info.sourceDir.equals("/packages/other/base.apk"));
        check(info.publicSourceDir.equals(info.sourceDir));
        check(info.splitSourceDirs != null && info.splitPublicSourceDirs != null);
        check(info.splitSourceDirs != info.splitPublicSourceDirs);
        check(info.splitSourceDirs.length == 2);
        check(info.splitNames.length == 2);
        check(info.splitNames[0].equals("config.foo"));
        check(info.splitNames[1].equals("config.arm64_v8a"));
        check(info.splitSourceDirs[0].equals("/packages/other/config.apk"));
        check(info.splitSourceDirs[1].equals("/packages/other/abi.apk"));
        info.splitSourceDirs[0] = "/foreign.apk";
        check(info.splitPublicSourceDirs[0].equals("/packages/other/config.apk"));
        check(info.dataDir.equals("/data/user/0/other"));
        check(info.deviceProtectedDataDir.equals("/data/user_de/0/other"));
        check(info.nativeLibraryDir.equals("/packages/other/android-elf/arm64-v8a"));
        check(info.uid == 10042);
        check(info.targetSdkVersion == 36);
        check((info.flags & ApplicationInfo.FLAG_HAS_CODE) == 0);
        check((info.flags & ApplicationInfo.FLAG_DEBUGGABLE) != 0);
        check((info.flags & ApplicationInfo.FLAG_SUPPORTS_RTL) != 0);
        String encodedMetadata =
                "4e554d5f50524956494c454745445f5345525649434553:i:0000002a,"
                + "666c6167:b:1,"
                + "74657874:s:68656c6c6f2c20ec84b8eab384,"
                + "656d707479:s:";
        ApplicationInfo withMetadata = InstalledApplicationInfo.fromRecord("other", record(
                "/packages/other/base.apk", null, "10042",
                "apk-app-runtime: package=other application=other.App application_metadata="
                        + encodedMetadata));
        check(withMetadata.metaData != null);
        check(withMetadata.metaData.getInt("NUM_PRIVILEGED_SERVICES") == 42);
        check(withMetadata.metaData.getBoolean("flag"));
        check(withMetadata.metaData.getString("text").equals("hello, 세계"));
        check(withMetadata.metaData.getString("empty").equals(""));
        ApplicationInfo legacy = InstalledApplicationInfo.fromRecord("other",
                record("/other.apk", null, null,
                        "apk-app-runtime: package=other application=none"));
        check(legacy.uid == -1 && legacy.className == null && legacy.targetSdkVersion == 1);
        check((legacy.flags & ApplicationInfo.FLAG_HAS_CODE) != 0);
        check((legacy.flags & ApplicationInfo.FLAG_SUPPORTS_RTL) == 0);
        check(legacy.metaData == null);
        try {
            InstalledApplicationInfo.fromRecord("wrong", record);
            throw new AssertionError("accepted mismatched package");
        } catch (IllegalArgumentException expected) {}
        try {
            InstalledApplicationInfo.fromRecord("other",
                    record.replace("target_sdk=36", "target_sdk=bad"));
            throw new AssertionError("silently accepted corrupt SDK");
        } catch (NumberFormatException expected) {}
        for (String malformed : new String[] {
                "split_names=config.foo", "split_names=config.foo,config.foo",
                "split_names=config.foo,", "no_split_names=missing"}) {
            try {
                InstalledApplicationInfo.fromRecord("other",
                        record.replace("split_names=config.foo,config.arm64_v8a", malformed));
                throw new AssertionError("accepted malformed split names: " + malformed);
            } catch (IllegalArgumentException expected) {}
        }
        String metadataPrefix = "apk-app-runtime: package=other application=none application_metadata=";
        for (String malformed : new String[] {
                "zz:i:00000001", "74657374:x:00000001", "74657374:i:1",
                "74657374:s:0", "74657374:s:ff", "74657374:i:00000001,74657374:b:0"}) {
            try {
                InstalledApplicationInfo.fromRecord("other", record(
                        "/other.apk", null, "10042", metadataPrefix + malformed));
                throw new AssertionError("accepted malformed application metadata: " + malformed);
            } catch (IllegalArgumentException expected) {}
        }
        check(InstalledApplicationInfo.fromRecord("missing", null) == null);
        System.out.println("InstalledApplicationInfo mapping PASS (legacy DTO fixture only)");
    }
}
