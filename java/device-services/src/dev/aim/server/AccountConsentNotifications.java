/*
 * Copyright (C) 2009 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */
package dev.aim.server;

import android.app.Notification;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.UserHandle;
import com.android.internal.notification.SystemNotificationChannels;

/** Presentation policy from android-16.0.0_r1 AccountManagerService (Apache-2.0).
 * Native accounts own decisions, request lifetime and the ordered show/cancel stream.
 */
final class AccountConsentNotifications extends IAccountConsentNotification.Stub {
    private final Context context;
    AccountConsentNotifications(Context context) { this.context = context; }
    @Override public synchronized void update(Intent intent, String tag, int uid, String accountName, String packageName) {
        UserHandle user = UserHandle.of(UserHandle.getUserId(uid));
        NotificationManager manager = context.getSystemService(NotificationManager.class);
        if (intent == null) { manager.cancelAsUser(tag, 38, user); return; }
        SystemNotificationChannels.createAccountChannelForPackage(packageName, uid, context);
        Context forUser = context.createContextAsUser(user, 0);
        PackageManager pm = context.getPackageManager();
        String label = packageName;
        try { label = pm.getApplicationLabel(pm.getApplicationInfoAsUser(packageName, 0, user.getIdentifier())).toString(); }
        catch (PackageManager.NameNotFoundException ignored) { }
        String text = context.getString(context.getResources().getIdentifier(
                "permission_request_notification_for_app_with_subtitle", "string", "android"), label, accountName);
        int split = text.indexOf('\n');
        String title = split > 0 ? text.substring(0, split) : text;
        String subtitle = split > 0 ? text.substring(split + 1) : "";
        Notification notification = new Notification.Builder(forUser, SystemNotificationChannels.ACCOUNT)
                .setSmallIcon(android.R.drawable.stat_sys_warning).setWhen(0)
                .setColor(forUser.getColor(forUser.getResources().getIdentifier(
                        "system_notification_accent_color", "color", "android")))
                .setContentTitle(title).setContentText(subtitle)
                .setContentIntent(PendingIntent.getActivityAsUser(context, 0, intent,
                        0x10000000 | PendingIntent.FLAG_IMMUTABLE, null, user)).build();
        manager.notifyAsUser(tag, 38, notification, user);
    }
}
