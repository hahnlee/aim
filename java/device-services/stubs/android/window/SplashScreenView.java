// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.window;

import android.content.Context;
import android.graphics.drawable.Drawable;
import android.os.RemoteCallback;
import android.view.SurfaceControlViewHost;
import android.widget.FrameLayout;

public final class SplashScreenView extends FrameLayout {
    public SplashScreenView(Context context) { super(null); throw new RuntimeException("stub"); }
    public boolean isCopyable() { throw new RuntimeException("stub"); }
    public void onCopied() { throw new RuntimeException("stub"); }
    public SurfaceControlViewHost getSurfaceHost() { throw new RuntimeException("stub"); }
    public static void releaseIconHost(SurfaceControlViewHost host) { throw new RuntimeException("stub"); }

    public interface IconAnimateListener {
        void prepareAnimate(java.util.function.LongConsumer startListener);
        void stopAnimation();
    }

    public static class Builder {
        public Builder(Context context) { throw new RuntimeException("stub"); }
        public Builder setIconSize(int iconSize) { throw new RuntimeException("stub"); }
        public Builder setIconBackground(Drawable iconBackground) { throw new RuntimeException("stub"); }
        public Builder setBackgroundColor(int backgroundColor) { throw new RuntimeException("stub"); }
        public Builder setOverlayDrawable(Drawable drawable) { throw new RuntimeException("stub"); }
        public Builder setCenterViewDrawable(Drawable drawable) { throw new RuntimeException("stub"); }
        public Builder setBrandingDrawable(Drawable branding, int width, int height) { throw new RuntimeException("stub"); }
        public Builder setAllowHandleSolidColor(boolean allowHandleSolidColor) { throw new RuntimeException("stub"); }
        public SplashScreenView build() { throw new RuntimeException("stub"); }
    }

    public static class SplashScreenViewParcelable {
        public SplashScreenViewParcelable(SplashScreenView view) { throw new RuntimeException("stub"); }
        public void setClientCallback(RemoteCallback clientCallback) { throw new RuntimeException("stub"); }
    }
}
