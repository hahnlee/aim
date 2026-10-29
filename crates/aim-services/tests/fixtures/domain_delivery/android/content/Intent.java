package android.content;
public final class Intent {
    public static final String ACTION_DOMAINS_NEED_VERIFICATION="android.intent.action.DOMAINS_NEED_VERIFICATION";
    public static final int FLAG_RECEIVER_FOREGROUND=0x10000000;
    public final String action; public ComponentName component; public int flags;
    public final java.util.Map<String,Object> extras=new java.util.HashMap<>();
    public Intent(String action){this.action=action;}
    public Intent setComponent(ComponentName value){component=value;return this;}
    public Intent putExtra(String key,android.os.Parcelable value){extras.put(key,value);return this;}
    public Intent addFlags(int value){flags|=value;return this;}
}
