package android.view;

import android.graphics.Rect;

public final class Gravity {
    public static final int START = 0x00800003;
    public static final int TOP = 0x30;
    public static void apply(int gravity, int width, int height, Rect container, int xAdj,
            int yAdj, Rect outRect) {
        outRect.left = container.left + xAdj;
        outRect.top = container.top + yAdj;
        outRect.right = outRect.left + width;
        outRect.bottom = outRect.top + height;
    }
    public static void applyDisplay(int gravity, Rect display, Rect inoutObj) {
        if (inoutObj.right > display.right) inoutObj.right = display.right;
        if (inoutObj.bottom > display.bottom) inoutObj.bottom = display.bottom;
    }
}
