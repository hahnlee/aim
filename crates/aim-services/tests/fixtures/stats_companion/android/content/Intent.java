// Host-only Android boundary double; production policy is compiled unchanged.
package android.content;
public class Intent {
    public static final String ACTION_PACKAGE_REPLACED="android.intent.action.PACKAGE_REPLACED",
        ACTION_PACKAGE_ADDED="android.intent.action.PACKAGE_ADDED",
        ACTION_PACKAGE_REMOVED="android.intent.action.PACKAGE_REMOVED",
        ACTION_USER_INITIALIZE="android.intent.action.USER_INITIALIZE",
        ACTION_USER_REMOVED="android.intent.action.USER_REMOVED",
        ACTION_REBOOT="android.intent.action.REBOOT",
        ACTION_SHUTDOWN="android.intent.action.ACTION_SHUTDOWN",
        EXTRA_REPLACING="android.intent.extra.REPLACING", EXTRA_UID="android.intent.extra.UID";
    public static final int FLAG_RECEIVER_FOREGROUND=0x10000000;
    public String action; public boolean replacing; public int flags;
    public final android.os.Bundle extras=new android.os.Bundle();
    public final android.net.Uri data=new android.net.Uri();
    public Intent(String action) { this.action=action; }
    public String getAction() { return action; }
    public boolean getBooleanExtra(String key,boolean fallback) { return replacing; }
    public android.os.Bundle getExtras() { return extras; }
    public android.net.Uri getData() { return data; }
    public int getFlags() { return flags; }
}
