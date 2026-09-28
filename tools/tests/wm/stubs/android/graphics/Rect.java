package android.graphics;
public final class Rect {
 public final int left, top, right, bottom;
 public Rect(int l,int t,int r,int b) { left=l; top=t; right=r; bottom=b; }
 public boolean isEmpty() { return right<=left || bottom<=top; }
}
