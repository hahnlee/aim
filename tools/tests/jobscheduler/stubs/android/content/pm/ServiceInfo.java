package android.content.pm;

public class ServiceInfo extends PackageItemInfo {
    public static final int FLAG_ISOLATED_PROCESS = 1 << 0;
    public String packageName;
    public String name;
    public String processName;
    public ApplicationInfo applicationInfo;
    public boolean enabled;
    public boolean exported;
    public String permission;
    public int flags;
}
