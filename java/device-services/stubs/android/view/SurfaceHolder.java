// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.view;

import android.graphics.Rect;

public interface SurfaceHolder {
    void setFixedSize(int width, int height);
    Surface getSurface();
    Rect getSurfaceFrame();
}
