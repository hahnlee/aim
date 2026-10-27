package com.android.server.vibrator;

import android.content.Context;
import android.os.Handler;
import android.os.Looper;
import android.os.VibrationAttributes;
import android.os.VibratorInfo;

/** The retained internal inputs of the native vibrator, android-16.0.0_r1. */
public final class NativeVibrationPolicy {
    private final VibrationSettings settings;
    private final HapticFeedbackVibrationProvider haptics;

    public NativeVibrationPolicy(Context context) {
        settings = new VibrationSettings(context, new Handler(Looper.getMainLooper()),
                new android.os.vibrator.VibrationConfig(context.getResources()));
        settings.onSystemReady();
        haptics = new HapticFeedbackVibrationProvider(context.getResources(), VibratorInfo.EMPTY_VIBRATOR_INFO);
    }

    public boolean shouldIgnore(int uid, int deviceId, VibrationAttributes attrs) {
        if (attrs.isFlagSet(VibrationAttributes.FLAG_INVALIDATE_SETTINGS_CACHE)) settings.update();
        return settings.shouldIgnoreVibration(
                new VibrationSession.CallerInfo(attrs, uid, deviceId, null, null)) != null;
    }

    public int[] hapticAttributes(int constant, int inputSource, int flags, int privFlags) {
        boolean input = inputSource != 0;
        if ((input ? haptics.getVibration(constant, inputSource) : haptics.getVibration(constant)) == null) {
            return null;
        }
        VibrationAttributes attrs = input
                ? haptics.getVibrationAttributes(constant, inputSource, flags, privFlags)
                : haptics.getVibrationAttributes(constant, flags, privFlags);
        return new int[] { attrs.getUsage(), attrs.getOriginalAudioUsage(), attrs.getFlags() };
    }
}
