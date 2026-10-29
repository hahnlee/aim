package android.content.res;
public class Resources {
    public static boolean show = true;
    public static Resources getSystem() { return new Resources(); }
    public boolean getBoolean(int id) { return show; }
}
