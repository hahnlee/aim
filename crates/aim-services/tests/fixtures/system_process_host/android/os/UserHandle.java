package android.os;
public class UserHandle {
    public static final int USER_SYSTEM = 0;
    public static int getUserId(int uid) { return uid / 100000; }
}
