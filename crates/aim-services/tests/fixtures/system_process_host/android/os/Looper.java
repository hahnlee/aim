package android.os;
public final class Looper {
    public static void prepareMainLooper() {}
    public static void loop() { throw new AssertionError("past attach boundary"); }
}
