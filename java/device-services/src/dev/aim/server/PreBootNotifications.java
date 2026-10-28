/*
 * Copyright (C) 2016 The Android Open Source Project
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 * http://www.apache.org/licenses/LICENSE-2.0
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */
package dev.aim.server;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.os.Handler;
import android.os.Looper;
import android.os.ServiceManager;
import android.os.UserHandle;
import com.android.internal.R;
import com.android.internal.notification.SystemNotificationChannels;

/**
 * PreBootBroadcaster's notification handler, frameworks/base at android-16.0.0_r1,
 * services/core/java/com/android/server/am/PreBootBroadcaster.java.
 * The app host supplies its receiver index; the system process owns the UI (#2006).
 */
public final class PreBootNotifications extends IPreBootNotifications.Stub {
    private static final String TAG = "PreBootBroadcaster";
    /** SystemMessage.NOTE_SYSTEM_UPGRADING (proto/src/system_messages.proto). */
    private static final int NOTE_SYSTEM_UPGRADING = 13;
    private final Context context;
    private final Handler handler = new Handler(Looper.getMainLooper());

    private PreBootNotifications(Context context) { this.context = context; }

    public static void publish(Context context) {
        // SystemNotificationChannels.createAll's UPDATES channel, also on fresh /data.
        context.getSystemService(NotificationManager.class).createNotificationChannel(
                new NotificationChannel(SystemNotificationChannels.UPDATES,
                        context.getString(R.string.notification_channel_updates),
                        NotificationManager.IMPORTANCE_LOW));
        ServiceManager.addService("aim.pre_boot_notifications", new PreBootNotifications(context));
    }

    @Override public void update(int userId, int total, int index) {
        Bridge.enforceSystemUid();
        handler.post(() -> {
            NotificationManager manager = context.getSystemService(NotificationManager.class);
            UserHandle user = UserHandle.of(userId);
            if (index >= total) {
                manager.cancelAsUser(TAG, NOTE_SYSTEM_UPGRADING, user);
                return;
            }
            CharSequence title = context.getText(R.string.android_upgrading_notification_title);
            Intent intent = new Intent();
            intent.setClassName("com.android.settings", "com.android.settings.HelpTrampoline");
            intent.putExtra("android.intent.extra.TEXT", "help_url_upgrading");
            PendingIntent contentIntent = context.getPackageManager().resolveActivity(intent, 0)
                    == null ? null : PendingIntent.getActivity(context, 0, intent,
                            PendingIntent.FLAG_IMMUTABLE);
            Notification notification = new Notification.Builder(context,
                    SystemNotificationChannels.UPDATES)
                    .setSmallIcon(R.drawable.stat_sys_adb)
                    .setWhen(0).setOngoing(true).setTicker(title)
                    .setColor(context.getColor(R.color.system_notification_accent_color))
                    .setContentTitle(title).setContentIntent(contentIntent)
                    .setVisibility(Notification.VISIBILITY_PUBLIC)
                    .setProgress(total, index, false).build();
            manager.notifyAsUser(TAG, NOTE_SYSTEM_UPGRADING, notification, user);
        });
    }
}
