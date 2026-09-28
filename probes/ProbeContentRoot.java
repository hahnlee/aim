package dev.aim.probe;

import android.content.Context;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;

/** Programmatic android.R.id.content parent inside the real DecorView. */
public final class ProbeContentRoot extends ViewGroup {
    public ProbeContentRoot(Context context) {
        super(context);
        setId(android.R.id.content);
    }

    @Override
    protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
        int width = MeasureSpec.getSize(widthMeasureSpec);
        int height = MeasureSpec.getSize(heightMeasureSpec);
        for (int index = 0; index < getChildCount(); index++) {
            View child = getChildAt(index);
            boolean materialButton = child instanceof Button;
            int childWidth = materialButton ? Math.min(width, 160) : width;
            int childHeight = materialButton ? Math.min(height, 56) : height;
            child.measure(
                    MeasureSpec.makeMeasureSpec(childWidth, MeasureSpec.EXACTLY),
                    MeasureSpec.makeMeasureSpec(childHeight, MeasureSpec.EXACTLY));
        }
        setMeasuredDimension(width, height);
    }

    @Override
    protected void onLayout(boolean changed, int left, int top, int right, int bottom) {
        int width = right - left;
        int height = bottom - top;
        for (int index = 0; index < getChildCount(); index++) {
            View child = getChildAt(index);
            if (child instanceof Button) {
                int childWidth = child.getMeasuredWidth();
                int childHeight = child.getMeasuredHeight();
                int childLeft = (width - childWidth) / 2;
                int childTop = (height - childHeight) / 2;
                child.layout(
                        childLeft, childTop, childLeft + childWidth, childTop + childHeight);
            } else {
                child.layout(0, 0, width, height);
            }
        }
    }
}
