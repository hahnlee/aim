package dev.aim.server;

import android.content.pm.UserInfo;
import android.os.IUserManager;
import android.os.RemoteException;
import android.os.UserHandle;
import android.os.UserManager;
import android.util.Slog;
import com.android.server.LocalServices;
import com.android.server.pm.UserManagerInternal;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/**
 * Pinned PackageManagerShellCommand.runRemoveUser against the original user owner.
 * Ported from AOSP android-16.0.0_r1, Apache License 2.0.
 */
final class PackageShellRemoveUserCommand extends android.os.ShellCommand {
    private static final String TAG = "PackageManager";
    @Override public int onCommand(String command) {
        if (!"remove-user".equals(command)) throw new IllegalArgumentException("Unknown user command");
        try { return runRemoveUser(); }
        catch (RemoteException error) { getOutPrintWriter().println("Remote exception: " + error); return -1; }
    }
    @Override public void onHelp() {}

    // pm remove-user [--set-ephemeral-if-in-use][--wait] USER_ID
    public int runRemoveUser() throws RemoteException {
        int userId;
        String arg;
        boolean setEphemeralIfInUse = false;
        boolean wait = false;

        while ((arg = getNextOption()) != null) {
            switch (arg) {
                case "--set-ephemeral-if-in-use":
                    setEphemeralIfInUse = true;
                    break;
                case "--wait": // fallthrough
                case "-w":
                    wait = true;
                    break;
                default:
                    getErrPrintWriter().println("Error: unknown option: " + arg);
                    return -1;
            }
        }

        arg = getNextArg();
        if (arg == null) {
            getErrPrintWriter().println("Error: no user id specified.");
            return 1;
        }
        userId = UserHandle.parseUserArg(arg);
        IUserManager um = com.android.server.pm.NativeUserManagerBridge.currentUserManagerOwner();
        if (setEphemeralIfInUse) {
            return removeUserWhenPossible(um, userId);
        } else {
            final boolean success = wait ? removeUserAndWait(um, userId) : removeUser(um, userId);
            if (success) {
                getOutPrintWriter().println("Success: removed user");
                return 0;
            } else {
                // Error message should already have been printed.
                return 1;
            }
        }
    }

    private boolean removeUser(IUserManager um, int userId) throws RemoteException {
        Slog.i(TAG, "Removing user " + userId);
        if (um.removeUser(userId)) {
            return true;
        } else {
            getErrPrintWriter().println("Error: couldn't remove user id " + userId);
            return false;
        }
    }

    private boolean removeUserAndWait(IUserManager um, int userId) throws RemoteException {
        Slog.i(TAG, "Removing (and waiting for completion) user " + userId);

        final CountDownLatch waitLatch = new CountDownLatch(1);
        final UserManagerInternal.UserLifecycleListener listener =
                new UserManagerInternal.UserLifecycleListener() {
                    @Override
                    public void onUserRemoved(UserInfo user) {
                        if (userId == user.id) {
                            waitLatch.countDown();
                        }
                    }
                };

        final UserManagerInternal umi = LocalServices.getService(UserManagerInternal.class);
        umi.addUserLifecycleListener(listener);

        try {
            if (um.removeUser(userId)) {
                final boolean awaitSuccess = waitLatch.await(10, TimeUnit.MINUTES);
                if (!awaitSuccess) {
                    getErrPrintWriter().printf("Error: Remove user %d timed out\n", userId);
                    return false;
                }
                // Success!
                return true;
            } else {
                getErrPrintWriter().println("Error: couldn't remove user id " + userId);
                return false;
            }
        } catch (InterruptedException e) {
            getErrPrintWriter().printf("Error: Remove user %d wait interrupted: %s\n", userId, e);
            Thread.currentThread().interrupt();
            return false;
        } finally {
            umi.removeUserLifecycleListener(listener);
        }
    }

    private int removeUserWhenPossible(IUserManager um, int userId) throws RemoteException {
        Slog.i(TAG, "Removing " + userId + " or set as ephemeral if in use.");
        int result = um.removeUserWhenPossible(userId, /* overrideDevicePolicy= */ false);
        switch (result) {
            case UserManager.REMOVE_RESULT_REMOVED:
                getOutPrintWriter().printf("Success: user %d removed\n", userId);
                return 0;
            case UserManager.REMOVE_RESULT_DEFERRED:
                getOutPrintWriter().printf("Success: user %d set as ephemeral\n", userId);
                return 0;
            case UserManager.REMOVE_RESULT_ALREADY_BEING_REMOVED:
                getOutPrintWriter().printf("Success: user %d is already being removed\n", userId);
                return 0;
            case UserManager.REMOVE_RESULT_ERROR_MAIN_USER_PERMANENT_ADMIN:
                getErrPrintWriter().printf("Error: user %d is a permanent admin main user\n",
                        userId);
                return 1;
            default:
                getErrPrintWriter().printf("Error: couldn't remove or mark ephemeral user id %d\n",
                        userId);
                return 1;
        }
    }
}
