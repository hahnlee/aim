package android.os;

/** Test stub: the system uid and the signal AppErrors sends for an ANR. */
public final class Process {
    public static final int SYSTEM_UID = 1000;
    public static final int SIGNAL_QUIT = 3;
    public static int lastSignalPid;
    public static int lastSignal;

    public static void sendSignal(int pid, int signal) {
        lastSignalPid = pid;
        lastSignal = signal;
    }
}
