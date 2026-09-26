package dev.darwinart.runtime.system;

import android.app.ActivityThread;
import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.drawable.Drawable;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.util.List;

/**
 * What a home screen asks PackageManager before it starts an app, for the
 * host launcher (ADR 0009): the launcher activity, the installed code, the
 * uid and the label and icon, all as PackageManagerService reports them.
 */
final class LauncherQueries {
    static final int EXIT_NOT_FOUND = 1;
    private static final int ICON_SIZE = 512;

    private LauncherQueries() {}

    private static PackageManager packageManager() {
        android.content.Context system = ActivityThread.currentActivityThread().getSystemContext();
        return system.getPackageManager();
    }

    /** One `key=value` line; values never span lines. */
    private static void line(ByteArrayOutputStream output, String key, Object value) {
        String text = key + "=" + String.valueOf(value).replace('\n', ' ').replace('\r', ' ')
                + "\n";
        byte[] bytes = text.getBytes(java.nio.charset.StandardCharsets.UTF_8);
        output.write(bytes, 0, bytes.length);
    }

    /** `launcher-info PACKAGE`: the launch facts for an installed package. */
    static int launcherInfo(String packageName, File iconDirectory,
            ByteArrayOutputStream output) throws IOException {
        PackageManager pm = packageManager();
        PackageInfo info;
        try {
            info = pm.getPackageInfo(packageName, 0);
        } catch (PackageManager.NameNotFoundException error) {
            line(output, "error", "package is not installed: " + packageName);
            return EXIT_NOT_FOUND;
        }
        ApplicationInfo app = info.applicationInfo;
        Intent launcher = new Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
                .setPackage(packageName);
        List<ResolveInfo> activities = pm.queryIntentActivities(launcher, 0);
        if (activities.isEmpty()) {
            line(output, "error", "package has no launcher activity: " + packageName);
            return EXIT_NOT_FOUND;
        }
        line(output, "package", packageName);
        line(output, "uid", app.uid);
        line(output, "activity", activities.get(0).activityInfo.name);
        line(output, "sourceDir", app.sourceDir);
        line(output, "splitSourceDirs",
                app.splitSourceDirs == null ? "" : String.join(":", app.splitSourceDirs));
        line(output, "nativeLibraryDir", app.nativeLibraryDir);
        line(output, "targetSdk", app.targetSdkVersion);
        line(output, "debuggable", (app.flags & ApplicationInfo.FLAG_DEBUGGABLE) != 0 ? 1 : 0);
        line(output, "versionCode", info.getLongVersionCode());
        line(output, "label", pm.getApplicationLabel(app));
        File icon = new File(iconDirectory, packageName + ".png");
        writeIcon(pm.getApplicationIcon(app), icon);
        line(output, "icon", icon.getPath());
        return 0;
    }

    private static void writeIcon(Drawable drawable, File file) throws IOException {
        Bitmap bitmap = Bitmap.createBitmap(ICON_SIZE, ICON_SIZE, Bitmap.Config.ARGB_8888);
        drawable.setBounds(0, 0, ICON_SIZE, ICON_SIZE);
        drawable.draw(new Canvas(bitmap));
        try (FileOutputStream stream = new FileOutputStream(file)) {
            if (!bitmap.compress(Bitmap.CompressFormat.PNG, 100, stream)) {
                throw new IOException("cannot encode icon " + file);
            }
        } finally {
            bitmap.recycle();
        }
    }

    /** `archive-info PATH`: the package an APK file declares. */
    static int archiveInfo(String path, ByteArrayOutputStream output) {
        PackageInfo info = packageManager().getPackageArchiveInfo(path, 0);
        if (info == null) {
            line(output, "error", "not a valid package archive: " + path);
            return EXIT_NOT_FOUND;
        }
        line(output, "package", info.packageName);
        line(output, "versionCode", info.getLongVersionCode());
        return 0;
    }
}
