package android.window;

import android.graphics.Rect;

/** Records the shell's calls, without a WindowManager or guest. */
public final class WindowContainerTransaction {
    public WindowContainerToken token;
    public int mode = -1, activityMode = -1;
    public Rect bounds;
    public WindowContainerTransaction setWindowingMode(WindowContainerToken t, int m) {
        token = t; mode = m; return this;
    }
    public WindowContainerTransaction setActivityWindowingMode(WindowContainerToken t, int m) {
        if (token != t) throw new AssertionError("activity token");
        activityMode = m; return this;
    }
    public WindowContainerTransaction setBounds(WindowContainerToken t, Rect b) {
        if (token != t) throw new AssertionError("bounds token");
        bounds = new Rect(b); return this;
    }
}
