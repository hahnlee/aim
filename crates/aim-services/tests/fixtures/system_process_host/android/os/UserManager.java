package android.os;
public final class UserManager {
    public android.content.pm.UserInfo getUserInfo(int user) {
        if (user != 0) throw new AssertionError(user);
        return null;
    }
}
