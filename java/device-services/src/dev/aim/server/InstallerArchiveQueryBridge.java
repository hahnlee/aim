package dev.aim.server;
import android.app.ActivityManager;
import android.app.AppOpsManager;
import android.content.Context;
import android.content.ComponentName;
import android.content.pm.LauncherApps;
import android.content.pm.PackageInstaller;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Rect;
import android.graphics.Color;
import android.graphics.PorterDuff;
import android.graphics.PorterDuffColorFilter;
import android.graphics.drawable.BitmapDrawable;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.LayerDrawable;
import android.os.Parcel;
import android.os.UserHandle;
import java.io.ByteArrayOutputStream;
import java.util.Objects;

/** Original image resources, LauncherApps and graphics; package state is native. */
public final class InstallerArchiveQueryBridge extends IInstallerArchiveQueryBridge.Stub {
    private final Context context;
    public InstallerArchiveQueryBridge(Context context) { this.context = Objects.requireNonNull(context); }
    private void enforce() { enforceNativeOwner(); }
    private Bitmap bitmap(Drawable icon) {
        if (icon == null) return null;
        int size = context.getSystemService(ActivityManager.class).getLauncherLargeIconSize();
        if (icon instanceof BitmapDrawable && ((BitmapDrawable) icon).getBitmap() != null
                && icon.getIntrinsicWidth() < size && icon.getIntrinsicHeight() < size) return ((BitmapDrawable) icon).getBitmap();
        int width = icon.getIntrinsicWidth(), height = icon.getIntrinsicHeight();
        if (width <= 0) width = size; if (height <= 0) height = size;
        float scale = Math.min(1f, (float) size / Math.max(width, height));
        Bitmap bitmap = Bitmap.createBitmap(Math.max(1, (int) (width * scale)), Math.max(1, (int) (height * scale)), Bitmap.Config.ARGB_8888);
        Rect saved = new Rect(icon.getBounds()); icon.setBounds(0, 0, bitmap.getWidth(), bitmap.getHeight());
        icon.draw(new Canvas(bitmap)); icon.setBounds(saved); return bitmap;
    }
    private byte[] png(Drawable drawable) {
        Bitmap bitmap = bitmap(drawable); if (bitmap == null) return null;
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        if (!bitmap.compress(Bitmap.CompressFormat.PNG, 100, bytes)) throw new IllegalStateException("archive icon encoding failed");
        return bytes.toByteArray();
    }
    @Override public byte[] getLauncherActivities(String name, int user) {
        enforce();
        var launcher = context.getSystemService(LauncherApps.class);
        if (launcher == null) throw new IllegalStateException("LauncherApps owner unavailable");
        var activities = launcher.getActivityList(name, UserHandle.of(user));
        Parcel out = Parcel.obtain();
        try {
            out.writeInt(1); out.writeInt(activities.size());
            for (var activity : activities) {
                out.writeString(activity.getLabel().toString());
                ComponentName component = activity.getComponentName(); out.writeString(component.getPackageName()); out.writeString(component.getClassName());
                out.writeByteArray(png(activity.getIcon(0)));
                Drawable mono = activity.getIcon(0) instanceof android.graphics.drawable.AdaptiveIconDrawable
                    ? ((android.graphics.drawable.AdaptiveIconDrawable) activity.getIcon(0)).getMonochrome() : null;
                out.writeByteArray(png(mono));
            }
            return out.marshall();
        } finally { out.recycle(); }
    }
    @Override public int getArchiveOverlayMode(int uid, String name) {
        enforce(); return context.getSystemService(AppOpsManager.class).checkOpNoThrow(AppOpsManager.OP_ARCHIVE_ICON_OVERLAY, uid, name);
    }
    @Override public int getArchiveOptOutMode(int uid, String name) {
        enforce(); return context.getSystemService(AppOpsManager.class).checkOpNoThrow(AppOpsManager.OP_AUTO_REVOKE_PERMISSIONS_IF_UNUSED, uid, name);
    }
    private byte[] encoded(Bitmap bitmap) {
        if (bitmap == null) return null;
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        if (!bitmap.compress(Bitmap.CompressFormat.PNG,100,output)) throw new IllegalStateException("archive overlay encoding failed");
        return output.toByteArray();
    }
    @Override public byte[] includeArchiveOverlay(Bitmap bitmap) {
        enforce();
        Drawable cloud = context.getResources().getDrawable(com.android.internal.R.drawable.archived_app_cloud_overlay, context.getTheme());
        if (cloud == null) return encoded(bitmap);
        BitmapDrawable icon = new BitmapDrawable(context.getResources(), bitmap);
        icon.setColorFilter(new PorterDuffColorFilter(Color.argb(0.5f, 0f, 0f, 0f), PorterDuff.Mode.SRC_ATOP));
        icon.setBounds(0, 0, cloud.getIntrinsicWidth(), cloud.getIntrinsicHeight());
        return encoded(bitmap(new LayerDrawable(new Drawable[]{icon, cloud})));
    }
    @Override public byte[] getUnarchiveDraftParams(String name, int user, int launcherUid, String launcherPackage) {
        enforce();
        var params = new PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL);
        params.setAppPackageName(name);
        params.setAppLabel(context.getString(com.android.internal.R.string.unarchival_session_app_label));
        // Icon bytes are supplied by the native archived Settings owner, so this
        // leaf does not query an original PackageArchiver or PMS instance.
        params.installFlags = (1 << 29) | (1 << 30);
        Parcel out = Parcel.obtain();
        try { params.writeToParcel(out, 0); return out.marshall(); }
        finally { out.recycle(); }
    }
    private static void enforceNativeOwner() {
        if (android.os.Binder.getCallingUid() != android.os.Process.SYSTEM_UID)
            throw new SecurityException("native installer bridge requires system UID");
    }
}
