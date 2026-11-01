package android.content;
public class Intent {
    public static final String ACTION_USER_SWITCHED="android.intent.action.USER_SWITCHED",
            ACTION_USER_REMOVED="android.intent.action.USER_REMOVED", ACTION_USER_STOPPED="android.intent.action.USER_STOPPED",
            ACTION_USER_UNLOCKED="android.intent.action.USER_UNLOCKED", EXTRA_USER_HANDLE="android.intent.extra.user_handle";
    public static final int FLAG_RECEIVER_REGISTERED_ONLY=0x40000000, FLAG_RECEIVER_FOREGROUND=0x10000000;
    private final String action;
    private int user = android.os.UserHandle.USER_NULL;
    public Intent(String action) { this.action = action; }
    public String getAction() { return action; }
    public int getIntExtra(String key, int fallback) { return user == android.os.UserHandle.USER_NULL ? fallback : user; }
    public Intent putExtra(String key, int user) { this.user = user; return this; }
    public Intent addFlags(int flags) { if (flags != (FLAG_RECEIVER_REGISTERED_ONLY|FLAG_RECEIVER_FOREGROUND)) throw new AssertionError(); return this; }
}
