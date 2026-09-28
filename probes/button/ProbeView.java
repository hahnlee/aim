package dev.aim.probe;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.RippleDrawable;
import android.util.Log;
import java.lang.reflect.Field;
import android.widget.Button;

/** Isolated real-widget acceptance class loaded in place of the baseline ProbeView. */
public final class ProbeView extends Button {
    private boolean presented;

    public ProbeView(Context context) {
        super(context);
        setClickable(true);
        setEnabled(true);
        setText("Click");
        // Retain the platform Button's themed background (the same Material
        // RippleDrawable a normal Android Button receives) and select the
        // hardware solid path for this standalone RenderNode capture.
        Drawable background = getBackground();
        configureSolidRipple(background);
    }

    private static void configureSolidRipple(Drawable background) {
        if (!(background instanceof RippleDrawable)) {
            return;
        }
        try {
            Field state = RippleDrawable.class.getDeclaredField("mState");
            state.setAccessible(true);
            Object rippleState = state.get(background);
            Field style = rippleState.getClass().getDeclaredField("mRippleStyle");
            style.setAccessible(true);
            // Android's Material default uses the patterned shader path.
            // Input is staged during the hardware RecordingCanvas traversal,
            // so this remains eligible for the RenderNode animation session.
            style.setInt(rippleState, 1);
        } catch (Throwable error) {
            Log.w("AimProbe", "could not select GPU solid ripple", error);
        }
    }

    @Override
    protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        if (!presented) {
            Log.i("AimProbe", "canvas=" + canvas.getClass().getName()
                    + " hardware=" + canvas.isHardwareAccelerated()
                    + " background=" + getBackground().getClass().getName());
        }
        presented = true;
    }

    public boolean wasPresented() {
        return presented;
    }
}
