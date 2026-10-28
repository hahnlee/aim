// Compile-only Android 16 API; checked against the original image at build time.
package android.app;

public class AlarmManager {
    AlarmManager(IAlarmManager service, android.content.Context ctx) { throw new RuntimeException("stub"); }
    public static final String ACTION_NEXT_ALARM_CLOCK_CHANGED = "android.app.action.NEXT_ALARM_CLOCK_CHANGED";
    public AlarmClockInfo getNextAlarmClock(int userId) { throw new RuntimeException("stub"); }
    public static final class AlarmClockInfo {
        public AlarmClockInfo(long triggerTime, PendingIntent showIntent) { throw new RuntimeException("stub"); }
        public PendingIntent getShowIntent() { throw new RuntimeException("stub"); }
    }
}
