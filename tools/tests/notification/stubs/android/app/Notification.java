package android.app;

/** Test stub: the fields NotificationRecords reads. */
public class Notification {
    public static final int FLAG_FOREGROUND_SERVICE = 0x00000040;
    public int flags;
    private final String channelId;

    public Notification(String channelId, int flags) {
        this.channelId = channelId;
        this.flags = flags;
    }

    public String getChannelId() { return channelId; }
}
