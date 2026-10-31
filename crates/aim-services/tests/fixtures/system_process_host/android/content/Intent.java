package android.content;
public class Intent {
    public static final String ACTION_USER_SWITCHED="android.intent.action.USER_SWITCHED", EXTRA_USER_HANDLE="android.intent.extra.user_handle";
    public static final int FLAG_RECEIVER_REGISTERED_ONLY=0x40000000, FLAG_RECEIVER_FOREGROUND=0x10000000;
    public Intent(String action) { if (!action.equals(ACTION_USER_SWITCHED)) throw new AssertionError(action); }
    public Intent putExtra(String key, int user) { if (!key.equals(EXTRA_USER_HANDLE) || user != 0) throw new AssertionError(); return this; }
    public Intent addFlags(int flags) { if (flags != (FLAG_RECEIVER_REGISTERED_ONLY|FLAG_RECEIVER_FOREGROUND)) throw new AssertionError(); return this; }
}
