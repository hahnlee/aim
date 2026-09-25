package dev.darwinart.runtime.pm;

import android.content.Intent;
import android.content.pm.ResolveInfo;
import java.util.List;

/** getLaunchIntentForPackage queries: MAIN/INFO first, then MAIN/LAUNCHER. */
public final class InstalledLaunchIntentsTest {
    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static String record(String metadata) {
        return "darwin-art-launch-v1\n"
                + "apk=/packages/game/base.apk\n"
                + "app_id=10042\n"
                + "metadata=" + metadata + "\n";
    }

    private static Intent main(String category, String packageName) {
        return new Intent(Intent.ACTION_MAIN).addCategory(category).setPackage(packageName);
    }

    public static void main(String[] args) {
        String metadata = "apk-app-runtime: package=game application=game.App "
                + "launch_component=game.Main "
                + "activities=game.Main=0x0,game.Info=0x0 "
                + "activity_aliases=none info_activities=none screen_orientation=-1";
        PackageRecords.Source packages = name -> "game".equals(name) ? record(metadata) : null;

        List<ResolveInfo> info = InstalledLaunchIntents.query(
                main(Intent.CATEGORY_INFO, "game"), null, 0, packages);
        check(info != null && info.isEmpty(), "no MAIN/INFO activity resolves to an empty list");
        List<ResolveInfo> launcher = InstalledLaunchIntents.query(
                main(Intent.CATEGORY_LAUNCHER, "game"), null, 0, packages);
        check(launcher.size() == 1, "one MAIN/LAUNCHER activity");
        check(launcher.get(0).activityInfo.name.equals("game.Main"), "launcher component");
        check(launcher.get(0).match == 0x108000, "category match without data");

        String withInfo = metadata.replace("info_activities=none", "info_activities=game.Info");
        PackageRecords.Source infoPackages = name -> "game".equals(name) ? record(withInfo) : null;
        info = InstalledLaunchIntents.query(
                main(Intent.CATEGORY_INFO, "game"), null, 0, infoPackages);
        check(info.size() == 1 && info.get(0).activityInfo.name.equals("game.Info"),
                "MAIN/INFO activity resolves");

        check(InstalledLaunchIntents.query(
                main(Intent.CATEGORY_LAUNCHER, "missing"), null, 0, packages).isEmpty(),
                "an absent package resolves nothing");
        // Schema 7 records do not describe INFO filters: unsupported, not empty.
        String legacy = metadata.replace(" info_activities=none", "");
        PackageRecords.Source legacyPackages = name -> record(legacy);
        check(InstalledLaunchIntents.query(
                main(Intent.CATEGORY_INFO, "game"), null, 0, legacyPackages) == null,
                "unknown INFO filters are unsupported");
        check(InstalledLaunchIntents.query(
                new Intent(Intent.ACTION_VIEW).setPackage("game"), null, 0, packages) == null,
                "other intents are outside the launch owner");
        check(InstalledLaunchIntents.query(
                main(Intent.CATEGORY_LAUNCHER, "game"), null, 0x00010000L, packages) == null,
                "MATCH_DEFAULT_ONLY needs CATEGORY_DEFAULT filter data");
        System.out.println("InstalledLaunchIntentsTest PASS");
    }
}
