package android.content.pm;

// Test-only DTO seam. Never included in runtime compilation or DEX packaging.
public final class ActivityInfo extends PackageItemInfo {
    public static final int FLAG_HARDWARE_ACCELERATED = 1 << 9;
    public String packageName, name, targetActivity;
    public ApplicationInfo applicationInfo;
    public boolean enabled;
    public int flags, theme, screenOrientation;
}
