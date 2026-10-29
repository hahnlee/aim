package dev.aim.server;

import android.content.Context;
import android.content.res.Resources;
import android.content.res.TypedArray;
import android.graphics.Rect;
import android.os.UserHandle;
import android.view.animation.Animation;
import android.view.animation.AnimationUtils;
import android.view.animation.Transformation;

/** The activity subset of Shell's DefaultTransitionHandler, android-16.0.0_r1. */
public final class ActivityAnimations extends IActivityAnimations.Stub {
    private final Context context;
    private static final String[] ATTRIBUTES = {"activityOpenEnterAnimation",
            "activityOpenExitAnimation", "activityCloseEnterAnimation", "activityCloseExitAnimation"};

    public ActivityAnimations(Context context) { this.context = context; }

    @Override
    public float[] sample(String packageName, int userId, int theme, int style, int attribute,
            int custom, int width, int height) {
        Bridge.enforceSystemUid();
        try {
            Context pkg = context.createPackageContextAsUser(packageName, 0, UserHandle.of(userId));
            pkg.setTheme(theme != 0 ? theme : Resources.selectDefaultTheme(
                    0, pkg.getApplicationInfo().targetSdkVersion));
            int id = custom;
            if (custom < 0) {
                if (style == 0) {
                    TypedArray window = pkg.obtainStyledAttributes(new int[] {
                            pkg.getResources().getIdentifier("windowAnimationStyle", "attr", "android")});
                    style = window.getResourceId(0, 0);
                    window.recycle();
                }
                if (style == 0) return new float[0];
                TypedArray animations = pkg.obtainStyledAttributes(style, new int[] {
                        pkg.getResources().getIdentifier(ATTRIBUTES[attribute], "attr", "android")});
                id = animations.getResourceId(0, 0);
                animations.recycle();
            }
            if (id == 0) return new float[0];
            Animation animation = AnimationUtils.loadAnimation(pkg, id);
            animation.initialize(width, height, width, height);
            animation.restrictDuration(3000);
            animation.scaleCurrentDuration(android.provider.Settings.Global.getFloat(
                    context.getContentResolver(), "transition_animation_scale", 1f));
            int duration = (int) Math.min(3000, animation.computeDurationHint());
            float[] samples = new float[(duration + 1) * 11];
            float[] matrix = new float[9];
            Transformation transform = new Transformation();
            animation.setStartTime(0);
            for (int ms = 0; ms <= duration; ms++) {
                transform.clear();
                animation.getTransformation(ms, transform);
                transform.getMatrix().getValues(matrix);
                int at = ms * 11;
                // SurfaceControl.Transaction.setMatrix: dsdx, dtdx, dtdy, dsdy.
                samples[at] = matrix[0]; samples[at + 1] = matrix[3];
                samples[at + 2] = matrix[4]; samples[at + 3] = matrix[1];
                samples[at + 4] = matrix[2]; samples[at + 5] = matrix[5];
                samples[at + 6] = transform.getAlpha();
                Rect clip = transform.hasClipRect() ? transform.getClipRect() : new Rect(0, 0, -1, -1);
                samples[at + 7] = clip.left; samples[at + 8] = clip.top;
                samples[at + 9] = clip.right; samples[at + 10] = clip.bottom;
            }
            return samples;
        } catch (android.content.pm.PackageManager.NameNotFoundException e) {
            throw new IllegalArgumentException(packageName, e);
        }
    }
}
