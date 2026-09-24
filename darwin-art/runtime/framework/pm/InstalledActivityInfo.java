package dev.darwinart.runtime.pm;

import android.content.pm.ActivityInfo;
import android.content.pm.ApplicationInfo;

/** Launch activity projection owned by installed package state. */
public final class InstalledActivityInfo {
    private InstalledActivityInfo() {}

    public static ActivityInfo launchActivity(String packageName, String record) {
        InstalledPackageRecord installed = InstalledPackageRecord.fromRecord(packageName, record);
        if (installed == null) return null;
        String activityName = installed.legacyManifestHint("launch_component");
        if (activityName == null || activityName.equals("none")) return null;

        return activity(packageName, record, activityName);
    }

    /** Resolves one manifest-declared activity from installed package state. */
    public static ActivityInfo activity(String packageName, String record, String activityName) {
        InstalledPackageRecord installed = InstalledPackageRecord.fromRecord(packageName, record);
        if (installed == null || activityName == null || activityName.isEmpty()) return null;

        String targetActivity = aliasTarget(installed, activityName);
        String declarationName = targetActivity == null ? activityName : targetActivity;
        int theme = 0;
        boolean declared = false;
        String activities = installed.legacyManifestHint("activities");
        if (activities != null && !activities.equals("none")) {
            for (String declaration : activities.split(",")) {
                int delimiter = declaration.lastIndexOf('=');
                if (delimiter > 0
                        && declarationName.equals(declaration.substring(0, delimiter))) {
                    theme = Integer.decode(declaration.substring(delimiter + 1));
                    declared = true;
                    break;
                }
            }
        }
        if (!declared) return null;

        ApplicationInfo application = InstalledApplicationInfo.fromRecord(packageName, record);
        ActivityInfo info = new ActivityInfo();
        info.packageName = packageName;
        info.name = activityName;
        info.targetActivity = targetActivity;
        info.applicationInfo = application;
        info.enabled = true;
        // These legacy metadata fields describe only the launch activity.
        // Other activities must retain normal ApplicationInfo label fallback
        // until their own manifest label is available in the package record.
        if (activityName.equals(installed.legacyManifestHint("launch_component"))) {
            InstalledApplicationInfo.applyLabel(
                    info, installed, "activity_label", "activity_label_res");
        }

        info.theme = theme;
        applyWindowPolicy(info, installed, declarationName, activityName);
        return info;
    }

    /**
     * Applies the declaring activity's own screenOrientation and configChanges.
     * Records from schema 5 carry one entry per activity; older records only
     * describe the launcher and must not leak its orientation to other activities.
     */
    private static void applyWindowPolicy(ActivityInfo info, InstalledPackageRecord installed,
            String declarationName, String activityName) {
        int orientation = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED;
        int configChanges = 0;
        boolean accelerated = false;
        String windows = installed.legacyManifestHint("activity_windows");
        if (windows != null) {
            if (!windows.equals("none")) {
                for (String declaration : windows.split(",")) {
                    int delimiter = declaration.lastIndexOf('=');
                    if (delimiter <= 0
                            || !declarationName.equals(declaration.substring(0, delimiter))) {
                        continue;
                    }
                    // orientation:configChanges:hardwareAccelerated
                    String[] fields = declaration.substring(delimiter + 1).split(":");
                    if (fields.length != 3) {
                        throw new IllegalArgumentException("malformed activity window record");
                    }
                    orientation = Integer.parseInt(fields[0]);
                    configChanges = Integer.decode(fields[1]);
                    accelerated = "1".equals(fields[2]);
                    break;
                }
            }
        } else if (activityName.equals(installed.legacyManifestHint("launch_component"))) {
            String legacy = installed.legacyManifestHint("screen_orientation");
            if (legacy != null) orientation = Integer.parseInt(legacy);
            accelerated = "1".equals(installed.legacyManifestHint("activity_hardware_accelerated"));
        }
        if (accelerated) info.flags |= ActivityInfo.FLAG_HARDWARE_ACCELERATED;
        info.screenOrientation = orientation;
        // PackageParser: recreateOnConfigChanges defaults to 0, so MCC/MNC are
        // handled by the Activity unless it explicitly asks to be recreated.
        info.configChanges = configChanges | ActivityInfo.CONFIG_MCC | ActivityInfo.CONFIG_MNC;
    }

    private static String aliasTarget(InstalledPackageRecord installed, String activityName) {
        String aliases = installed.legacyManifestHint("activity_aliases");
        if (aliases == null || aliases.equals("none")) return null;
        for (String declaration : aliases.split(",")) {
            int delimiter = declaration.indexOf('>');
            if (delimiter > 0 && activityName.equals(declaration.substring(0, delimiter))) {
                String target = declaration.substring(delimiter + 1);
                return target.isEmpty() ? null : target;
            }
        }
        return null;
    }
}
