package dev.darwinart.runtime.pm;

import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.ActivityInfo;
import android.content.pm.ResolveInfo;
import java.util.ArrayList;
import java.util.List;
import java.util.Set;

/**
 * Activity resolution for the launch queries PackageManager issues itself:
 * ACTION_MAIN with exactly CATEGORY_INFO or CATEGORY_LAUNCHER inside one
 * package, as ApplicationPackageManager.getLaunchIntentForPackage does. The
 * installed record carries those two filter shapes only, so any other intent
 * is reported as unsupported rather than resolved against partial filters.
 */
final class InstalledLaunchIntents {
    // PackageManager.MATCH_DEFAULT_ONLY: needs CATEGORY_DEFAULT filter data.
    private static final long MATCH_DEFAULT_ONLY = 0x00010000L;

    private InstalledLaunchIntents() {}

    /** Returns the resolved activities, or null when the query is outside this owner. */
    static List<ResolveInfo> query(Intent intent, String resolvedType, long flags,
            PackageRecords.Source packages) {
        if (intent == null || resolvedType != null || (flags & MATCH_DEFAULT_ONLY) != 0
                || !Intent.ACTION_MAIN.equals(intent.getAction())
                || intent.getData() != null || intent.getType() != null
                || intent.getComponent() != null || intent.getSelector() != null
                || intent.getPackage() == null) {
            return null;
        }
        Set<String> categories = intent.getCategories();
        if (categories == null || categories.size() != 1) return null;
        String category = categories.iterator().next();
        String packageName = intent.getPackage();
        String record = packages.resolveInstalledPackage(packageName);
        List<ResolveInfo> result = new ArrayList<>();
        InstalledPackageRecord installed = InstalledPackageRecord.fromRecord(packageName, record);
        if (installed == null) return result;
        List<String> names = new ArrayList<>();
        if (Intent.CATEGORY_LAUNCHER.equals(category)) {
            String launcher = installed.legacyManifestHint("launch_component");
            if (launcher != null && !launcher.equals("none")) names.add(launcher);
        } else if (Intent.CATEGORY_INFO.equals(category)) {
            String info = installed.legacyManifestHint("info_activities");
            if (info == null) return null;
            if (!info.equals("none")) {
                for (String name : info.split(",")) names.add(name);
            }
        } else {
            return null;
        }
        for (String name : names) {
            ActivityInfo activity = InstalledActivityInfo.activity(packageName, record, name);
            if (activity == null) continue;
            ResolveInfo resolved = new ResolveInfo();
            resolved.activityInfo = activity;
            // IntentFilter.match for an action and category without data.
            resolved.match = IntentFilter.MATCH_CATEGORY_EMPTY
                    | IntentFilter.MATCH_ADJUSTMENT_NORMAL;
            result.add(resolved);
        }
        return result;
    }
}
