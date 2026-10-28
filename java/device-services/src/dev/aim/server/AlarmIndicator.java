package dev.aim.server;

import android.app.AlarmManager;
import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.os.Handler;
import android.os.Looper;
import android.os.RemoteCallbackList;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.UserHandle;
import android.util.Slog;

/** NextAlarmController's broadcast/query path, android-16.0.0_r1 (#591). */
public final class AlarmIndicator extends IAlarmIndicator.Stub {
    private final Context context;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final RemoteCallbackList<IAlarmIndicatorListener> listeners = new RemoteCallbackList<>();

    private AlarmIndicator(Context context) {
        this.context = context;
        context.registerReceiverAsUser(new BroadcastReceiver() {
            @Override public void onReceive(Context ignored, Intent intent) { update(); }
        }, UserHandle.SYSTEM, new IntentFilter(AlarmManager.ACTION_NEXT_ALARM_CLOCK_CHANGED),
                null, handler);
    }

    public static void publish(Context context) {
        ServiceManager.addService("aim.alarm_indicator", new AlarmIndicator(context));
    }

    @Override public void watch(IAlarmIndicatorListener listener) {
        Bridge.enforceSystemUid();
        if (listener == null) throw new IllegalArgumentException("No alarm listener");
        handler.post(() -> {
            listeners.register(listener);
            update();
        });
    }

    private void update() {
        AlarmManager.AlarmClockInfo alarm = context.getSystemService(AlarmManager.class)
                .getNextAlarmClock(UserHandle.USER_SYSTEM);
        PendingIntent show = alarm == null ? null : alarm.getShowIntent();
        String creator = show == null ? null : show.getCreatorPackage();
        String owner = alarm == null ? null : creator == null ? "android" : creator;
        int count = listeners.beginBroadcast();
        try {
            for (int i = 0; i < count; i++) {
                try {
                    listeners.getBroadcastItem(i).changed(owner);
                } catch (RemoteException e) {
                    Slog.w("AimAlarmIndicator", "Cannot report next alarm", e);
                }
            }
        } finally {
            listeners.finishBroadcast();
        }
    }
}
