import android.content.pm.ServiceInfo;
import dev.darwinart.runtime.pm.InstalledServiceInfo;

public final class InstalledServiceInfoTest {
    private static void check(boolean value) {
        if (!value) throw new AssertionError();
    }

    private static String record(String metadata) {
        return "darwin-art-launch-v1\n"
                + "apk=/packages/browser/base.apk\n"
                + "app_id=10042\n"
                + "metadata=" + metadata + "\n";
    }

    private static void rejects(String metadata, String name) {
        try {
            InstalledServiceInfo.service("browser", record(metadata), name);
            throw new AssertionError("accepted malformed service metadata");
        } catch (IllegalArgumentException expected) {}
    }

    public static void main(String[] args) {
        String metadata = "apk-app-runtime: package=browser application=browser.App "
                + "services=browser.Renderer>browser:renderer>1>636f6d2e6578616d706c652e42494e445f4a4f425f53455256494345>1>1,"
                + "browser.Main>browser>0>none>0>1,"
                + "browser.Disabled>browser>0>none>0>0";
        ServiceInfo renderer = InstalledServiceInfo.service(
                "browser", record(metadata), "browser.Renderer");
        check(renderer != null);
        check(renderer.packageName.equals("browser"));
        check(renderer.name.equals("browser.Renderer"));
        check(renderer.processName.equals("browser:renderer"));
        check(renderer.applicationInfo.packageName.equals("browser"));
        check(renderer.enabled);
        check(renderer.exported);
        check(renderer.permission.equals("com.example.BIND_JOB_SERVICE"));
        check((renderer.flags & ServiceInfo.FLAG_ISOLATED_PROCESS) != 0);
        ServiceInfo main = InstalledServiceInfo.service(
                "browser", record(metadata), "browser.Main");
        check(main != null);
        check(main.enabled);
        check(!main.exported);
        check(main.permission == null);
        check((main.flags & ServiceInfo.FLAG_ISOLATED_PROCESS) == 0);
        ServiceInfo disabled = InstalledServiceInfo.service(
                "browser", record(metadata), "browser.Disabled");
        check(disabled != null);
        check(!disabled.enabled);
        check(!disabled.exported);
        ServiceInfo legacy = InstalledServiceInfo.service(
                "browser", record("apk-app-runtime: package=browser "
                        + "services=browser.Legacy>browser:legacy"), "browser.Legacy");
        check(legacy != null);
        check(legacy.processName.equals("browser:legacy"));
        check((legacy.flags & ServiceInfo.FLAG_ISOLATED_PROCESS) == 0);
        check(legacy.enabled);
        check(!legacy.exported);
        check(legacy.permission == null);
        rejects("apk-app-runtime: package=browser services=browser.Bad>browser>1>none>1", "browser.Bad");
        rejects("apk-app-runtime: package=browser services=browser.Bad>browser>2", "browser.Bad");
        rejects("apk-app-runtime: package=browser services=browser.Bad>browser>0>0>1>1", "browser.Bad");
        rejects("apk-app-runtime: package=browser services=browser.Bad>browser>0>0g>1>1", "browser.Bad");
        check(InstalledServiceInfo.service(
                "browser", record(metadata), "browser.Missing") == null);
        System.out.println("InstalledServiceInfo mapping PASS");
    }
}
