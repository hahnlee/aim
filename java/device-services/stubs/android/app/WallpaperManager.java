// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app;

import android.graphics.drawable.Drawable;

public class WallpaperManager {
    public static final int FLAG_SYSTEM = 1;
    public static final int FLAG_LOCK = 2;

    WallpaperManager() { throw new RuntimeException("stub"); }
    public Drawable getDrawable(int which) { throw new RuntimeException("stub"); }
    public void forgetLoadedWallpaper() { throw new RuntimeException("stub"); }
}
