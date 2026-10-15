// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.graphics.drawable;

import android.graphics.Canvas;
import android.graphics.ColorFilter;
import android.graphics.Rect;

public abstract class Drawable {
    public int getIntrinsicHeight(){throw new RuntimeException("stub");}
    public int getIntrinsicWidth(){throw new RuntimeException("stub");}
    public Drawable() { throw new RuntimeException("stub"); }
    public abstract void draw(Canvas canvas);
    public abstract void setAlpha(int alpha);
    public abstract int getOpacity();
    public abstract void setColorFilter(ColorFilter colorFilter);
    public void setBounds(Rect bounds) { throw new RuntimeException("stub"); }
    protected void onBoundsChange(Rect bounds) { throw new RuntimeException("stub"); }
    public void invalidateSelf() { throw new RuntimeException("stub"); }
    public void scheduleSelf(Runnable what, long when) { throw new RuntimeException("stub"); }
    public void unscheduleSelf(Runnable what) { throw new RuntimeException("stub"); }
    public final void setCallback(Callback cb) { throw new RuntimeException("stub"); }

    public interface Callback {
        void invalidateDrawable(Drawable who);
        void scheduleDrawable(Drawable who, Runnable what, long when);
        void unscheduleDrawable(Drawable who, Runnable what);
    }
 public final android.graphics.Rect getBounds(){throw new RuntimeException("stub");}
 public void setBounds(int left,int top,int right,int bottom){throw new RuntimeException("stub");}
}
