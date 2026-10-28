package android.os;
public final class Looper {
    public static void prepareMainLooper() {}
    public static Looper getMainLooper() { return null; }
    public static void loop() { throw new dev.aim.server.SystemProcessHostTest.Attached(); }
}
