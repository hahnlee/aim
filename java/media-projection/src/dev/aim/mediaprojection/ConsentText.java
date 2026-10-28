package dev.aim.mediaprojection;

import android.content.Context;
import android.content.res.AssetManager;
import android.content.res.Resources;

/** Android selects and formats the original SystemUI translations (#590). */
final class ConsentText {
    private ConsentText() {}

    static String[] forApp(Context context, String label) {
        AssetManager assets = new AssetManager();
        try {
            if (assets.addAssetPath("/system/framework/aim-shell-res.apk") == 0) {
                throw new IllegalStateException("Cannot load the original shell resources");
            }
            Resources current = context.getResources();
            Resources resources = new Resources(assets, current.getDisplayMetrics(),
                    current.getConfiguration());
            return new String[] {
                text(resources, "media_projection_entry_app_permission_dialog_title", label),
                text(resources, "media_projection_entry_app_permission_dialog_warning_entire_screen",
                        label),
                text(resources, "media_projection_entry_app_permission_dialog_continue_entire_screen"),
                text(resources, "cancel"),
            };
        } finally {
            assets.close();
        }
    }

    private static String text(Resources resources, String name, Object... args) {
        int id = resources.getIdentifier(name, "string", "com.android.systemui");
        if (id == 0) throw new Resources.NotFoundException(name);
        return resources.getString(id, args);
    }
}
