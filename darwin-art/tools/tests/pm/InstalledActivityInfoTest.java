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
        System.out.println("InstalledActivityInfo alias mapping PASS");
    }
}
