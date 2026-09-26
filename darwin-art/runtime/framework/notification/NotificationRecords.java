package dev.darwinart.runtime.notification;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.os.UserHandle;
import android.service.notification.StatusBarNotification;
import android.util.Log;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * NotificationManagerService state for posted notifications and the channel
 * preferences each app declares (PreferencesHelper). Records are keyed by the
 * calling uid, so one app can neither see nor cancel another app's entries.
 * There is no status bar listener; posted notifications are held as active
 * records exactly as NMS holds them for its listeners.
 */
final class NotificationRecords {
    private static final String TAG = "NotificationService";

    private static final class Posted {
        final String tag;
        final int id;
        final int userId;
        final int pid;
        final Notification notification;
        final long postTime;

        Posted(String tag, int id, int userId, int pid, Notification notification) {
            this.tag = tag;
            this.id = id;
            this.userId = userId;
            this.pid = pid;
            this.notification = notification;
            postTime = System.currentTimeMillis();
        }

        boolean matches(String otherTag, int otherId, int otherUser) {
            return Objects.equals(tag, otherTag) && id == otherId && userId == otherUser;
        }
    }

    private static final class App {
        final Map<String, NotificationChannel> channels = new LinkedHashMap<>();
        final List<Posted> posted = new ArrayList<>();
    }

    private final Map<String, App> apps = new LinkedHashMap<>();

    private App app(int uid, String packageName, boolean create) {
        if (packageName == null) throw new IllegalArgumentException("package is null");
        String key = uid + "/" + packageName;
        App app = apps.get(key);
        if (app == null && create) {
            app = new App();
            apps.put(key, app);
        }
        return app;
    }

    /** PreferencesHelper.createNotificationChannel for each declared channel. */
    synchronized void createChannels(int uid, String packageName, List<?> channels) {
        App app = app(uid, packageName, true);
        for (Object item : channels) {
            NotificationChannel channel = (NotificationChannel) item;
            int importance = channel.getImportance();
            if (importance < NotificationManager.IMPORTANCE_NONE
                    || importance > NotificationManager.IMPORTANCE_MAX) {
                throw new IllegalArgumentException("Invalid importance level");
            }
            NotificationChannel existing = app.channels.get(channel.getId());
            if (existing == null) {
                app.channels.put(channel.getId(), channel);
                continue;
            }
            // An app may rename or re-describe its channel and may lower, but
            // never raise, its importance; other settings belong to the user.
            existing.setName(channel.getName());
            if (channel.getDescription() != null) {
                existing.setDescription(channel.getDescription());
            }
            if (existing.getGroup() == null) existing.setGroup(channel.getGroup());
            if (importance < existing.getImportance()) existing.setImportance(importance);
        }
    }

    synchronized List<NotificationChannel> channels(int uid, String packageName) {
        App app = app(uid, packageName, false);
        return app == null ? new ArrayList<>() : new ArrayList<>(app.channels.values());
    }

    synchronized NotificationChannel channel(int uid, String packageName, String channelId) {
        App app = app(uid, packageName, false);
        return app == null || channelId == null ? null : app.channels.get(channelId);
    }

    /** Deleting a channel cancels its notifications (NMS.deleteNotificationChannel). */
    synchronized void deleteChannel(int uid, String packageName, String channelId) {
        App app = app(uid, packageName, false);
        if (app == null || app.channels.remove(channelId) == null) return;
        app.posted.removeIf(posted -> channelId.equals(posted.notification.getChannelId()));
    }

    /**
     * NMS.enqueueNotificationInternal: a notification whose channel the app has
     * not created is rejected with an error log, not an exception. A repost
     * with the same tag, id and user replaces the active record.
     */
    synchronized void enqueue(int uid, int pid, String packageName, String tag, int id,
            Notification notification, int userId) {
        if (notification == null) throw new IllegalArgumentException("null not allowed");
        App app = app(uid, packageName, true);
        String channelId = notification.getChannelId();
        if (channelId == null || !app.channels.containsKey(channelId)) {
            Log.e(TAG, "No Channel found for pkg=" + packageName + ", channelId=" + channelId
                    + ", id=" + id + ", tag=" + tag + ", opPkg=" + packageName
                    + ", callingUid=" + uid + ", userId=" + userId);
            return;
        }
        app.posted.removeIf(posted -> posted.matches(tag, id, userId));
        app.posted.add(new Posted(tag, id, userId, pid, notification));
    }

    synchronized void cancel(int uid, String packageName, String tag, int id, int userId) {
        App app = app(uid, packageName, false);
        if (app != null) app.posted.removeIf(posted -> posted.matches(tag, id, userId));
    }

    /** cancelAllNotifications keeps foreground-service notifications. */
    synchronized void cancelAll(int uid, String packageName, int userId) {
        App app = app(uid, packageName, false);
        if (app == null) return;
        for (Iterator<Posted> it = app.posted.iterator(); it.hasNext();) {
            Posted posted = it.next();
            if (posted.userId == userId && (posted.notification.flags
                    & Notification.FLAG_FOREGROUND_SERVICE) == 0) {
                it.remove();
            }
        }
    }

    synchronized List<StatusBarNotification> active(int uid, String packageName, int userId) {
        List<StatusBarNotification> result = new ArrayList<>();
        App app = app(uid, packageName, false);
        if (app == null) return result;
        for (Posted posted : app.posted) {
            if (posted.userId != userId) continue;
            // NotificationRecord.sbn: no group key override here.
            result.add(new StatusBarNotification(packageName, packageName, posted.id, posted.tag,
                    uid, posted.pid, posted.notification, UserHandle.of(posted.userId),
                    null, posted.postTime));
        }
        return result;
    }
}
