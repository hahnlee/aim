// Compile-only Android 16 API; checked against the original image at build time.
package android.app;

public class AlarmManager {
    public static final String ACTION_NEXT_ALARM_CLOCK_CHANGED = "android.app.action.NEXT_ALARM_CLOCK_CHANGED";
    public AlarmClockInfo getNextAlarmClock(int userId) { throw new RuntimeException("stub"); }
    public static final class AlarmClockInfo {
        public PendingIntent getShowIntent() { throw new RuntimeException("stub"); }
    }
}
