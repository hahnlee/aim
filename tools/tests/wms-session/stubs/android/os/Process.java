package android.os;

/** Fixed system-server identity for Binder callback admission tests. */
public final class Process {
    private Process() {}
    public static int myPid() { return 5000; }
    public static int myUid() { return 1000; }
}
