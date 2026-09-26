package dev.darwinart.runtime.wm;

import android.content.pm.ActivityInfo;
import android.content.res.Resources;
import android.content.res.TypedArray;
import android.os.Build;
import android.os.UserHandle;
import com.android.internal.R;
import com.android.internal.policy.AttributeCache;

/**
 * ActivityRecord's window style: whether an Activity's theme makes it occlude
 * the Activities behind it, read through the system process's AttributeCache
 * as ActivityTaskManagerService.getWindowStyle does.
 */
final class ActivityWindowStyle {
    private ActivityWindowStyle() {}

    /**
     * ActivityRecord.mOccludesParent: not translucent and not floating, or
     * showing the wallpaper. A theme that cannot be read occludes, as in
     * ActivityRecord when no window style is found.
     */
    static boolean occludesParent(ActivityInfo info) {
        if (info == null || info.applicationInfo == null) return true;
        int theme = info.getThemeResource();
        if (theme == Resources.ID_NULL) {
            theme = info.applicationInfo.targetSdkVersion < Build.VERSION_CODES.HONEYCOMB
                    ? android.R.style.Theme : android.R.style.Theme_Holo;
        }
        AttributeCache cache = AttributeCache.instance();
        AttributeCache.Entry entry = cache == null ? null : cache.get(info.packageName, theme,
                R.styleable.Window, UserHandle.getUserId(info.applicationInfo.uid));
        if (entry == null) return true;
        TypedArray style = entry.array;
        return !(style.getBoolean(R.styleable.Window_windowIsTranslucent, false)
                        || style.getBoolean(R.styleable.Window_windowIsFloating, false))
                || style.getBoolean(R.styleable.Window_windowShowWallpaper, false);
    }
}
