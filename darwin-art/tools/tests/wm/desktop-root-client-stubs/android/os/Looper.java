package android.os;

public final class Looper {
    private static Looper main;
    public static Looper getMainLooper() { return main; }
    public static void setMainLooper(Looper value) { main = value; }
}
