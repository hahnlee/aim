package android.os;
// Test-only authenticated caller boundary for the actual admission owner.
public final class Binder {
    public static int caller = 100;
    public static int getCallingPid() { return caller; }
}
