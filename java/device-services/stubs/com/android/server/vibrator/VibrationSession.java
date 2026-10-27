package com.android.server.vibrator;
import android.os.VibrationAttributes;
interface VibrationSession {
    enum Status { UNKNOWN(0); Status(int code) {} }
    final class CallerInfo {
        CallerInfo(VibrationAttributes attrs, int uid, int deviceId, String opPkg, String reason) {}
    }
}
