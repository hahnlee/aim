package dev.darwinart.runtime.notification;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.service.notification.StatusBarNotification;
import android.util.Log;
import java.util.Arrays;
import java.util.List;

/** NotificationManagerService record and PreferencesHelper channel rules. */
public final class NotificationRecordsTest {
    private static final int APP = 10001;
    private static final int OTHER = 10002;

    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static void channelRules() {
        NotificationRecords records = new NotificationRecords();
        records.createChannels(APP, "app", Arrays.asList(
                new NotificationChannel("news", "News", NotificationManager.IMPORTANCE_DEFAULT)));
        NotificationChannel update =
                new NotificationChannel("news", "Headlines", NotificationManager.IMPORTANCE_HIGH);
        update.setDescription("Top stories");
        records.createChannels(APP, "app", Arrays.asList(update));
        NotificationChannel stored = records.channel(APP, "app", "news");
        check("Headlines".equals(stored.getName().toString()), "channel was not renamed");
        check("Top stories".equals(stored.getDescription()), "description was not updated");
        check(stored.getImportance() == NotificationManager.IMPORTANCE_DEFAULT,
                "an app raised its channel importance");
        records.createChannels(APP, "app", Arrays.asList(
                new NotificationChannel("news", "Headlines", NotificationManager.IMPORTANCE_LOW)));
        check(records.channel(APP, "app", "news").getImportance()
                        == NotificationManager.IMPORTANCE_LOW, "an app could not lower importance");
        try {
            records.createChannels(APP, "app", Arrays.asList(new NotificationChannel("bad", "Bad", 9)));
            throw new AssertionError("invalid importance accepted");
        } catch (IllegalArgumentException expected) {
        }
        check(records.channels(OTHER, "app").isEmpty(), "another uid saw the app's channels");
    }

    private static void postingRules() {
        NotificationRecords records = new NotificationRecords();
        int errors = Log.errors;
        records.enqueue(APP, 1, "app", null, 1, new Notification("missing", 0), 0);
        check(Log.errors == errors + 1, "a notification without a channel was not rejected");
        check(records.active(APP, "app", 0).isEmpty(), "a channel-less notification was posted");

        records.createChannels(APP, "app", Arrays.asList(
                new NotificationChannel("news", "News", NotificationManager.IMPORTANCE_DEFAULT),
                new NotificationChannel("music", "Music", NotificationManager.IMPORTANCE_LOW)));
        records.enqueue(APP, 1, "app", "a", 1, new Notification("news", 0), 0);
        records.enqueue(APP, 1, "app", "a", 1, new Notification("news", 0), 0);
        records.enqueue(APP, 1, "app", "b", 2, new Notification("music",
                Notification.FLAG_FOREGROUND_SERVICE), 0);
        List<StatusBarNotification> active = records.active(APP, "app", 0);
        check(active.size() == 2, "a repost did not replace its record: " + active.size());
        check(records.active(OTHER, "app", 0).isEmpty(), "another uid saw the app's notifications");

        records.cancel(OTHER, "app", "a", 1, 0);
        check(records.active(APP, "app", 0).size() == 2, "another uid cancelled a notification");
        records.cancelAll(APP, "app", 0);
        active = records.active(APP, "app", 0);
        check(active.size() == 1 && active.get(0).getId() == 2,
                "cancelAll removed a foreground-service notification or kept another");

        records.deleteChannel(APP, "app", "music");
        check(records.active(APP, "app", 0).isEmpty(),
                "deleting a channel kept its notifications");
        check(records.channel(APP, "app", "music") == null, "deleted channel remained");
    }

    public static void main(String[] args) {
        channelRules();
        postingRules();
        System.out.println("notification-records: channel and posting rules PASS");
    }
}
