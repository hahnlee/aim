package android.content.pm;

public class ServiceInfo extends ComponentInfo {
    public static final int FLAG_ISOLATED_PROCESS = 1;
    public boolean exported;
    public String permission;
    public String processName;
    public int flags;
}
