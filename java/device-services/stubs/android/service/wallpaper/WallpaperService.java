// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.service.wallpaper;

import android.app.Service;
import android.app.WallpaperColors;
import android.view.SurfaceHolder;

public abstract class WallpaperService extends Service {
    public WallpaperService() { throw new RuntimeException("stub"); }
    public abstract Engine onCreateEngine();

    public class Engine {
        public Engine() { throw new RuntimeException("stub"); }
        public void setFixedSizeAllowed(boolean allowed) { throw new RuntimeException("stub"); }
        public int getWallpaperFlags() { throw new RuntimeException("stub"); }
        public void onCreate(SurfaceHolder surfaceHolder) { throw new RuntimeException("stub"); }
        public void onSurfaceRedrawNeeded(SurfaceHolder holder) { throw new RuntimeException("stub"); }
        public WallpaperColors onComputeColors() { throw new RuntimeException("stub"); }
        public void notifyColorsChanged() { throw new RuntimeException("stub"); }
    }
}
