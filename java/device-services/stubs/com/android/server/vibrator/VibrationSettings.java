package com.android.server.vibrator;
import android.content.Context;
import android.os.Handler;
final class VibrationSettings {
    VibrationSettings(Context context, Handler handler, android.os.vibrator.VibrationConfig config) {}
    public void onSystemReady() {}
    void update() {}
    public VibrationSession.Status shouldIgnoreVibration(VibrationSession.CallerInfo caller) { return null; }
}
