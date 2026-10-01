// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.view;

import android.content.Context;

public abstract class ViewGroup extends View {
    public ViewGroup(Context context) { super(null); throw new RuntimeException("stub"); }
    public void addView(View child) { throw new RuntimeException("stub"); }

    public static class LayoutParams {
        public LayoutParams(int width, int height) { throw new RuntimeException("stub"); }
    }
}
