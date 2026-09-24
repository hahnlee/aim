import android.content.pm.ActivityInfo;
import dev.darwinart.runtime.pm.InstalledActivityInfo;

public final class InstalledActivityInfoTest {
    private static void check(boolean value) {
        if (!value) throw new AssertionError();
    }

    private static String record(String metadata) {
        return "darwin-art-launch-v1\n"
                + "apk=/packages/browser/base.apk\n"
                + "app_id=10042\n"
                + "metadata=" + metadata + "\n";
    }

    public static void main(String[] args) {
        String metadata = "apk-app-runtime: package=browser application=browser.App "
                + "launch_component=browser.Main "
                + "activities=browser.Real=0x7f010042 "
                + "activity_aliases=browser.Main>browser.Real "
                + "activity_hardware_accelerated=1 screen_orientation=-1";
        ActivityInfo alias = InstalledActivityInfo.launchActivity(
                "browser", record(metadata));
        check(alias != null);
        check(alias.name.equals("browser.Main"));
        check(alias.targetActivity.equals("browser.Real"));
        check(alias.theme == 0x7f010042);
        check(alias.screenOrientation == -1);
        check((alias.flags & ActivityInfo.FLAG_HARDWARE_ACCELERATED) != 0);

        ActivityInfo target = InstalledActivityInfo.activity(
                "browser", record(metadata), "browser.Real");
        check(target != null && target.name.equals("browser.Real"));
        check(target.targetActivity == null);
        check(InstalledActivityInfo.activity(
                "browser", record(metadata), "browser.Missing") == null);
        check(target.screenOrientation == -1);
        check(target.configChanges == 0x3);

        // Schema 5: each Activity keeps its own orientation/configChanges; the
        // launcher's orientation must not be applied to other activities.
        String windows = "apk-app-runtime: package=game application=game.App "
                + "launch_component=game.Main "
                + "activities=game.Main=0x0,game.Web=0x0 "
                + "activity_windows=game.Main=11:0x40003fb4:1,game.Web=-1:0x0:0 "
                + "activity_aliases=none screen_orientation=11";
        ActivityInfo main = InstalledActivityInfo.launchActivity("game", record(windows));
        check(main.screenOrientation == 11);
        check(main.configChanges == (0x40003fb4 | 0x3));
        check((main.flags & ActivityInfo.FLAG_HARDWARE_ACCELERATED) != 0);
        ActivityInfo web = InstalledActivityInfo.activity("game", record(windows), "game.Web");
        check(web.screenOrientation == -1);
        check((web.flags & ActivityInfo.FLAG_HARDWARE_ACCELERATED) == 0);
        check(web.configChanges == 0x3);

        // Pre-schema-5 records describe only the launcher.
        String legacy = windows.replace(
                "activity_windows=game.Main=11:0x40003fb4:1,game.Web=-1:0x0:0 ", "");
        check(InstalledActivityInfo.launchActivity("game", record(legacy))
                .screenOrientation == 11);
        check(InstalledActivityInfo.activity("game", record(legacy), "game.Web")
                .screenOrientation == -1);
        System.out.println("InstalledActivityInfo alias mapping PASS");
    }
}
