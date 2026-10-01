package dev.aim.wallpaper;

import android.app.WallpaperColors;
import android.app.WallpaperManager;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.drawable.BitmapDrawable;
import android.graphics.drawable.Drawable;
import android.service.wallpaper.WallpaperService;
import android.view.Surface;
import android.view.SurfaceHolder;

/**
 * The static wallpaper, drawn as SystemUI's ImageWallpaper draws it: the
 * current bitmap of the engine's screen (the lock screen's for a
 * lock-only engine, else the home screen's, else the default), on a
 * surface of the bitmap's size that WindowManager places, and its colors.
 * WallpaperManagerService binds it again whenever the bitmap changes.
 */
public final class ImageWallpaper extends WallpaperService {
    /** ImageWallpaper's smallest surface. */
    private static final int MIN_SIZE = 128;

    @Override
    public Engine onCreateEngine() {
        return new ImageEngine();
    }

    private final class ImageEngine extends Engine {
        private WallpaperManager mManager;
        private WallpaperColors mColors;

        ImageEngine() {
            setFixedSizeAllowed(true);
        }

        @Override
        public void onCreate(SurfaceHolder holder) {
            mManager = getSystemService(WallpaperManager.class);
            Bitmap bitmap = bitmap();
            if (bitmap != null) {
                holder.setFixedSize(Math.max(MIN_SIZE, bitmap.getWidth()),
                        Math.max(MIN_SIZE, bitmap.getHeight()));
                mColors = WallpaperColors.fromBitmap(bitmap);
                // As ImageWallpaper does once it has the bitmap's colors:
                // WallpaperManagerService takes them from the engine.
                notifyColorsChanged();
            }
        }

        @Override
        public void onSurfaceRedrawNeeded(SurfaceHolder holder) {
            Bitmap bitmap = bitmap();
            if (bitmap == null) {
                return;
            }
            Surface surface = holder.getSurface();
            Canvas canvas = surface.lockHardwareCanvas();
            try {
                canvas.drawBitmap(bitmap, null, holder.getSurfaceFrame(), null);
            } finally {
                surface.unlockCanvasAndPost(canvas);
            }
            // Nothing redraws it until the surface changes: the bitmap
            // need not stay in memory.
            mManager.forgetLoadedWallpaper();
        }

        @Override
        public WallpaperColors onComputeColors() {
            return mColors;
        }

        private Bitmap bitmap() {
            int which = getWallpaperFlags() == WallpaperManager.FLAG_LOCK
                    ? WallpaperManager.FLAG_LOCK
                    : WallpaperManager.FLAG_SYSTEM;
            Drawable d = mManager.getDrawable(which);
            return d instanceof BitmapDrawable ? ((BitmapDrawable) d).getBitmap() : null;
        }
    }
}
