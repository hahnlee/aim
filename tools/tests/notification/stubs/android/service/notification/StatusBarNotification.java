package android.service.notification;

import android.app.Notification;
import android.os.UserHandle;

/** Test stub: the hidden constructor NotificationRecords uses. */
public class StatusBarNotification {
    private final String packageName;
    private final int id;
    private final String tag;
    private final int uid;
    private final Notification notification;

    public StatusBarNotification(String pkg, String opPkg, int id, String tag, int uid,
            int initialPid, Notification notification, UserHandle user,
            String overrideGroupKey, long postTime) {
        this.packageName = pkg;
        this.id = id;
        this.tag = tag;
        this.uid = uid;
        this.notification = notification;
    }

    public String getPackageName() { return packageName; }
    public int getId() { return id; }
    public String getTag() { return tag; }
    public int getUid() { return uid; }
    public Notification getNotification() { return notification; }
}
