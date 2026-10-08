// Icon storage ported from AOSP PackageArchiver, android-16.0.0_r1, Apache 2.0.
package com.android.server.pm;

import android.content.Context;
import android.content.pm.ArchivedActivityInfo;
import android.content.pm.ArchivedPackageInfo;
import android.content.pm.ArchivedPackageParcel;
import android.graphics.Bitmap;
import android.graphics.drawable.AdaptiveIconDrawable;
import android.graphics.drawable.BitmapDrawable;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.InsetDrawable;
import android.os.Environment;
import android.os.SELinux;
import com.android.server.pm.pkg.ArchiveState;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.file.Path;
import java.util.ArrayList;

final class NativeArchiveGraphics {
    private NativeArchiveGraphics() {}
    static ArchiveState store(Context context, ArchivedPackageParcel parcel, int user, String title) {
        int size = context.getSystemService(android.app.ActivityManager.class).getLauncherLargeIconSize();
        ArchivedPackageInfo info = new ArchivedPackageInfo(parcel);
        var result = new ArrayList<ArchiveState.ArchiveActivityInfo>();
        try {
            var activities = info.getLauncherActivities();
            for (int i = 0; i < activities.size(); i++) {
                var activity = activities.get(i);
                Path icon = store(info.getPackageName(), activity.getIcon(), user, i * 2, size);
                Path monochrome = store(info.getPackageName(), activity.getMonochromeIcon(), user, i * 2 + 1, size);
                result.add(new ArchiveState.ArchiveActivityInfo(activity.getLabel().toString(),
                        activity.getComponentName(), icon, monochrome));
            }
            return new ArchiveState(result, title);
        } catch (IOException failure) {
            android.util.Slog.e("PackageArchiverService", "Failed to create archive state", failure);
            return null;
        }
    }
    private static final class FixedBitmap extends BitmapDrawable {
        FixedBitmap(Bitmap bitmap) { super(null, bitmap); }
        @Override public int getIntrinsicHeight() { return getBitmap().getWidth(); }
        @Override public int getIntrinsicWidth() { return getBitmap().getWidth(); }
    }
    private static Path store(String name, Drawable drawable, int user, int index, int size) throws IOException {
        if (drawable == null) return null;
        if (drawable instanceof BitmapDrawable bitmap) drawable = new FixedBitmap(bitmap.getBitmap());
        float inset = AdaptiveIconDrawable.getExtraInsetFraction(); inset = inset / (1 + 2 * inset);
        drawable = new AdaptiveIconDrawable(new ColorDrawable(0xff000000), new InsetDrawable(drawable, inset, inset, inset, inset));
        File directory = new File(new File(Environment.getDataSystemCeDirectory(user), "package_archiver"), name);
        if (!directory.isDirectory()) {
            directory.delete(); directory.mkdirs();
            if (!directory.isDirectory()) throw new IOException("Unable to create directory " + directory);
        }
        SELinux.restorecon(directory);
        File file = new File(directory, index + ".png");
        Bitmap bitmap = ArchivedActivityInfo.drawableToBitmap(drawable, size);
        try (FileOutputStream output = new FileOutputStream(file)) {
            if (!bitmap.compress(Bitmap.CompressFormat.PNG, 100, output)) throw new IOException("Failure to store icon file " + file);
            output.flush();
        }
        return file.toPath();
    }
}
