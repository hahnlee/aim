package dev.aim.server;
import android.content.pm.UserInfo;
import android.os.Binder;
import android.os.UserManager;
import android.provider.Settings;
import android.content.res.Resources;

public final class UserSwitcherPolicyTest {
    static final UserSwitcherPolicy policy = new UserSwitcherPolicy(new android.content.Context());
    static final UserInfo[] one = {new UserInfo(true)}, two = {new UserInfo(true), new UserInfo(true)};
    static boolean enabled(boolean show, boolean noSwitch, boolean noAdd, boolean read, boolean query, UserInfo[] users) {
        return policy.isEnabled(show, noSwitch, noAdd, read, query, users);
    }
    static void check(boolean condition) { if (!condition) throw new AssertionError(); }
    static void denied(Runnable call, String message) {
        try { call.run(); throw new AssertionError("permission accepted"); }
        catch (SecurityException expected) { check(expected.getMessage().contains(message)); }
    }
    public static void main(String[] args) {
        check(enabled(false, false, false, true, false, one));
        check(!enabled(false, false, true, true, true, one));
        check(enabled(false, false, true, true, true, two));
        check(!enabled(false, false, true, true, true, new UserInfo[] {one[0], new UserInfo(false)}));
        check(enabled(true, false, true, true, false, one));
        denied(() -> enabled(false, false, true, true, false, two), "CREATE_USERS");
        denied(() -> enabled(true, false, false, false, true, one), "INTERACT_ACROSS_USERS");
        check(!enabled(true, true, false, true, false, two));
        UserManager.multiple = false;
        check(!enabled(true, false, false, false, false, two));
        UserManager.multiple = true;
        UserManager.demo = true;
        check(!enabled(true, false, false, true, false, two));
        UserManager.demo = false;
        Settings.Global.setting = 0;
        check(!enabled(true, false, false, true, false, two));
        denied(() -> enabled(true, false, false, false, true, one), "INTERACT_ACROSS_USERS");
        Settings.Global.setting = null;
        Resources.show = false;
        check(!enabled(true, false, false, true, false, two));
        Settings.Global.setting = 1;
        check(enabled(true, false, false, true, false, two));
        for (int uid : new int[] {0, 2000, 10100, 101000}) {
            Binder.uid = uid;
            denied(() -> enabled(true, false, false, true, true, two), "system UID");
        }
        Binder.uid = 1000;
    }
}
