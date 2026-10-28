package android.os;
public class Binder {
    public static int uid = 1000, cleared;
    public static int getCallingUid() { return uid; }
    public static long clearCallingIdentity() { cleared++; return 1; }
    public static void restoreCallingIdentity(long token) { cleared--; }
}
