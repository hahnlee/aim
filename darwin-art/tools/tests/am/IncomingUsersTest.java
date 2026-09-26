package dev.darwinart.runtime.am;

import android.os.UserHandle;
import java.util.HashSet;
import java.util.Set;

/** IncomingUsers: UserController.handleIncomingUser's rules for user 0. */
public final class IncomingUsersTest {
    private static final int APP = 10042;
    private static final String ACROSS = android.Manifest.permission.INTERACT_ACROSS_USERS;
    private static final String ACROSS_FULL =
            android.Manifest.permission.INTERACT_ACROSS_USERS_FULL;

    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static <T extends Throwable> void expect(Class<T> type, Runnable call, String message) {
        try {
            call.run();
        } catch (Throwable error) {
            if (type.isInstance(error)) return;
            throw new AssertionError(message + ": threw " + error, error);
        }
        throw new AssertionError(message + ": did not throw");
    }

    public static void main(String[] args) {
        Set<String> granted = new HashSet<>();
        IncomingUsers users = new IncomingUsers((uid, permission) ->
                granted.contains(uid + ":" + permission));

        check(users.handle(APP, 0, false, false, "t") == 0, "own user");
        check(users.handle(APP, UserHandle.USER_CURRENT, false, false, "t") == 0,
                "current resolves to user 0");
        check(users.handle(APP, UserHandle.USER_CURRENT_OR_SELF, false, true, "t") == 0,
                "current-or-self resolves to user 0");

        // USER_ALL needs allowAll and a cross-user grant.
        expect(SecurityException.class,
                () -> users.handle(APP, UserHandle.USER_ALL, true, false, "t"),
                "all users without a grant");
        granted.add(APP + ":" + ACROSS);
        check(users.handle(APP, UserHandle.USER_ALL, true, false, "t") == UserHandle.USER_ALL,
                "all users with INTERACT_ACROSS_USERS");
        expect(SecurityException.class,
                () -> users.handle(APP, UserHandle.USER_ALL, true, true, "t"),
                "full-only needs INTERACT_ACROSS_USERS_FULL");
        granted.add(APP + ":" + ACROSS_FULL);
        check(users.handle(APP, UserHandle.USER_ALL, true, true, "t") == UserHandle.USER_ALL,
                "all users with INTERACT_ACROSS_USERS_FULL");
        expect(IllegalArgumentException.class,
                () -> users.handle(APP, UserHandle.USER_ALL, false, false, "t"),
                "special user without allowAll");

        // System callers need no grant; a special user still needs allowAll.
        check(users.handle(1000, UserHandle.USER_ALL, true, true, "t") == UserHandle.USER_ALL,
                "system uid");
        check(users.handle(0, 10, false, true, "t") == 10, "root names any user");
        expect(IllegalArgumentException.class,
                () -> users.handle(1000, UserHandle.USER_ALL, false, true, "t"),
                "system caller, special user without allowAll");

        // Another user's uid asking for user 0 without a grant.
        int otherUserApp = 10 * UserHandle.PER_USER_RANGE + 42;
        expect(SecurityException.class,
                () -> users.handle(otherUserApp, 0, false, false, "t"),
                "cross-user without a grant");
        check(users.handle(otherUserApp, UserHandle.USER_CURRENT_OR_SELF, false, false, "t") == 10,
                "current-or-self falls back to the caller's user");
        System.out.println("IncomingUsersTest passed");
    }
}
