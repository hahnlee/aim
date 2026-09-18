package android.content.res;

public class Resources {
    public static Resources getSystem() { return new Resources(); }
    public Configuration getConfiguration() { return new Configuration(); }
    public DisplayMetrics getDisplayMetrics() { return new DisplayMetrics(); }
    public static final class DisplayMetrics { public float density = 1.0f; }
}
