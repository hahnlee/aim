package android.app;

/** Test stub: the error-state record AppErrors builds. */
public class ActivityManager {
    public static class ProcessErrorStateInfo {
        public static final int CRASHED = 1;
        public int condition;
        public String processName;
        public int pid;
        public int uid;
        public String tag;
        public String shortMsg;
        public String longMsg;
        public String stackTrace;
    }
}
