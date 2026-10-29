package dev.aim.server;
final class UserSwitcherPolicy extends android.os.Binder {
    UserSwitcherPolicy(android.content.Context context) {
        if (context == null) throw new AssertionError("missing user switcher context");
    }
}
