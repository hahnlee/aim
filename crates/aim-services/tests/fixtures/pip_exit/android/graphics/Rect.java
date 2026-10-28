package android.graphics;

/** Host test value, never packaged in the image. */
public final class Rect {
    public int left, top, right, bottom;
    public Rect(int l, int t, int r, int b) { left = l; top = t; right = r; bottom = b; }
    public Rect(Rect r) { this(r.left, r.top, r.right, r.bottom); }
}
