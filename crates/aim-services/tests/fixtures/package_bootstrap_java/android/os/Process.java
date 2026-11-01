package android.os;
public final class Process {
    public static int kills;
    public static int myPid() { return 123; }
    public static int myUid() { return 1000; }
    public static void killProcess(int pid) { if (pid != myPid()) throw new AssertionError(pid); kills++; }
    public static boolean isIsolatedUid(int uid) { return uid % 100000 >= 99000; }
    public static final int ROOT_UID = 0, SYSTEM_UID = 1000, SHELL_UID = 2000, THREAD_PRIORITY_BACKGROUND = 10;
}
