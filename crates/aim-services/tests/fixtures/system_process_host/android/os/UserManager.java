package android.os;
public final class UserManager {
    public static android.content.pm.UserInfo info;
    public static int requested;
    public android.content.pm.UserInfo getUserInfo(int user) {
        requested = user;
        return info;
    }
}
