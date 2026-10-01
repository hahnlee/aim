package dev.aim.server;

import android.content.res.Resources;
import android.graphics.Canvas;
import android.graphics.ColorFilter;
import android.graphics.Matrix;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.PixelFormat;
import android.graphics.Rect;
import android.graphics.drawable.AdaptiveIconDrawable;
import android.graphics.drawable.Animatable;
import android.graphics.drawable.AnimatedVectorDrawable;
import android.graphics.drawable.AnimationDrawable;
import android.graphics.drawable.Drawable;
import android.util.PathParser;
import android.util.Slog;
import android.window.SplashScreenView;

import java.util.function.LongConsumer;

/**
 * The splash screen icon's drawables, ported from WMShell's
 * SplashscreenIconDrawableFactory (Apache-2.0): the icon background masked
 * by the device's icon shape, and an animated icon
 * (`windowSplashScreenAnimatedIcon` that is Animatable) the splash screen
 * plays and hands over to its app.
 */
final class SplashIcons {
    private static final String TAG = "AimWindowShell";

    private SplashIcons() {}

    /** A fill of `color` in the icon shape (config_icon_mask). */
    static class MaskBackground extends Drawable {
        private static final float MASK_SIZE = AdaptiveIconDrawable.MASK_SIZE;
        private static final float EXTRA_INSET_PERCENTAGE = 1 / 4f;
        static final float DEFAULT_VIEW_PORT_SCALE = 1f / (1 + 2 * EXTRA_INSET_PERCENTAGE);

        private final Path mMask;
        private final Path mMaskScaleOnly;
        private final Matrix mMaskMatrix = new Matrix();
        private final Paint mBackgroundPaint;

        MaskBackground(int color) {
            mMask = PathParser.createPathFromPathData(Resources.getSystem()
                    .getString(com.android.internal.R.string.config_icon_mask));
            mMaskScaleOnly = new Path(mMask);
            if (color != 0) {
                mBackgroundPaint = new Paint(
                        Paint.ANTI_ALIAS_FLAG | Paint.DITHER_FLAG | Paint.FILTER_BITMAP_FLAG);
                mBackgroundPaint.setColor(color);
                mBackgroundPaint.setStyle(Paint.Style.FILL);
            } else {
                mBackgroundPaint = null;
            }
        }

        @Override
        protected void onBoundsChange(Rect bounds) {
            if (!bounds.isEmpty()) {
                updateLayerBounds(bounds);
            }
        }

        protected void updateLayerBounds(Rect bounds) {
            mMaskMatrix.setScale(bounds.width() / MASK_SIZE, bounds.height() / MASK_SIZE);
            mMask.transform(mMaskMatrix, mMaskScaleOnly);
        }

        @Override
        public void draw(Canvas canvas) {
            canvas.clipPath(mMaskScaleOnly);
            if (mBackgroundPaint != null) {
                canvas.drawPath(mMaskScaleOnly, mBackgroundPaint);
            }
        }

        @Override
        public void setAlpha(int alpha) {
            if (mBackgroundPaint != null) {
                mBackgroundPaint.setAlpha(alpha);
            }
        }

        @Override
        public int getOpacity() {
            return PixelFormat.RGBA_8888;
        }

        @Override
        public void setColorFilter(ColorFilter colorFilter) {}
    }

    /** `foreground` in the icon shape, inset as an adaptive icon's foreground. */
    static class MaskedForeground extends MaskBackground {
        protected final Drawable mForeground;
        private final Rect mTmpOutRect = new Rect();

        MaskedForeground(Drawable foreground) {
            super(0);
            mForeground = foreground;
        }

        @Override
        protected void updateLayerBounds(Rect bounds) {
            super.updateLayerBounds(bounds);
            int cX = bounds.width() / 2;
            int cY = bounds.height() / 2;
            int insetWidth = (int) (bounds.width() / (DEFAULT_VIEW_PORT_SCALE * 2));
            int insetHeight = (int) (bounds.height() / (DEFAULT_VIEW_PORT_SCALE * 2));
            mTmpOutRect.set(cX - insetWidth, cY - insetHeight, cX + insetWidth, cY + insetHeight);
            mForeground.setBounds(mTmpOutRect);
            invalidateSelf();
        }

        @Override
        public void draw(Canvas canvas) {
            super.draw(canvas);
            mForeground.draw(canvas);
        }

        @Override
        public void setColorFilter(ColorFilter colorFilter) {
            mForeground.setColorFilter(colorFilter);
        }
    }

    /**
     * An Animatable icon, which the splash screen view hosts in a surface
     * of its own and starts at its first draw.
     */
    static final class AnimatedIcon extends MaskedForeground
            implements SplashScreenView.IconAnimateListener {
        private final Animatable mAnimatable;
        private boolean mAnimationTriggered;
        private boolean mRunning;
        private LongConsumer mStartListener;

        AnimatedIcon(Drawable foreground) {
            super(foreground);
            mForeground.setCallback(new Callback() {
                @Override
                public void invalidateDrawable(Drawable who) {
                    invalidateSelf();
                }

                @Override
                public void scheduleDrawable(Drawable who, Runnable what, long when) {
                    scheduleSelf(what, when);
                }

                @Override
                public void unscheduleDrawable(Drawable who, Runnable what) {
                    unscheduleSelf(what);
                }
            });
            mAnimatable = (Animatable) foreground;
        }

        @Override
        public void prepareAnimate(LongConsumer startListener) {
            stopAnimation();
            mStartListener = startListener;
        }

        private void startAnimation() {
            try {
                mAnimatable.start();
            } catch (RuntimeException e) {
                Slog.e(TAG, "the splash screen's animated icon failed", e);
                mRunning = false;
                if (mStartListener != null) {
                    mStartListener.accept(0);
                }
                return;
            }
            long duration = 0;
            if (mAnimatable instanceof AnimatedVectorDrawable avd && avd.getTotalDuration() > 0) {
                duration = avd.getTotalDuration();
            } else if (mAnimatable instanceof AnimationDrawable ad && ad.getTotalDuration() > 0) {
                duration = ad.getTotalDuration();
            }
            mRunning = true;
            if (mStartListener != null) {
                mStartListener.accept(duration);
            }
        }

        @Override
        public void stopAnimation() {
            if (mRunning) {
                mAnimatable.stop();
                mStartListener = null;
                mRunning = false;
            }
        }

        @Override
        public void draw(Canvas canvas) {
            if (!mAnimationTriggered) {
                if (!mRunning) {
                    startAnimation();
                }
                mAnimationTriggered = true;
            }
            super.draw(canvas);
        }
    }
}
