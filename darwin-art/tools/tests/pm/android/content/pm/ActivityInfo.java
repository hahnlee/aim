package android.content.pm;

// Test-only DTO seam. Never included in runtime compilation or DEX packaging.
public final class ActivityInfo extends PackageItemInfo {
    public static final int FLAG_HARDWARE_ACCELERATED = 1 << 9;
    public static final int SCREEN_ORIENTATION_UNSPECIFIED = -1;
    public static final int CONFIG_MCC = 0x0001;
    public static final int CONFIG_MNC = 0x0002;
    public String packageName, name, targetActivity;
    public ApplicationInfo applicationInfo;
    public boolean enabled;
    public int flags, theme, screenOrientation, configChanges;
}
