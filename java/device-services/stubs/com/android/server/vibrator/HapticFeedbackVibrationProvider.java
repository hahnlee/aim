package com.android.server.vibrator;
import android.content.res.Resources;
import android.os.VibrationAttributes;
import android.os.VibrationEffect;
import android.os.VibratorInfo;
public final class HapticFeedbackVibrationProvider {
    public HapticFeedbackVibrationProvider(Resources res, VibratorInfo info) {}
    public VibrationEffect getVibration(int effect) { return null; }
    public VibrationEffect getVibration(int effect, int source) { return null; }
    public VibrationAttributes getVibrationAttributes(int effect, int flags, int privFlags) { return null; }
    public VibrationAttributes getVibrationAttributes(int effect, int source, int flags, int privFlags) { return null; }
}
