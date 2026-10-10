package dev.aim.server;

import android.accounts.IAccountManager;
import android.app.ActivityManager;
import android.content.pm.UserInfo;
import android.os.IUserManager;
import android.os.Process;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.ServiceSpecificException;
import android.os.Trace;
import android.os.UserHandle;
import android.os.UserManager;

/** Pinned PackageManagerShellCommand.runCreateUser against the original user owner. */
final class PackageShellCreateUserCommand extends android.os.ShellCommand {
    @Override public int onCommand(String command) {
        if (!"create-user".equals(command)) throw new IllegalArgumentException("Unknown user command");
        try { return runCreateUser(); }
        catch (RemoteException error) { getOutPrintWriter().println("Remote exception: " + error); return -1; }
    }
    @Override public void onHelp() {}
    private int translateUserId(int userId, int allUserId, String logContext) {
        int translated = ActivityManager.handleIncomingUser(android.os.Binder.getCallingPid(),
                android.os.Binder.getCallingUid(), userId, allUserId != UserHandle.USER_NULL,
                true, logContext, "pm command");
        return translated == UserHandle.USER_ALL ? allUserId : translated;
    }
    public int runCreateUser() throws RemoteException {
        String name;
        int userId = -1;
        int flags = 0;
        String userType = null;
        String opt;
        boolean preCreateOnly = false;
        while ((opt = getNextOption()) != null) {
            String newUserType = null;
            if ("--profileOf".equals(opt)) {
                userId = translateUserId(UserHandle.parseUserArg(getNextArgRequired()),
                            UserHandle.USER_ALL, "runCreateUser");
            } else if ("--managed".equals(opt)) {
                newUserType = UserManager.USER_TYPE_PROFILE_MANAGED;
            } else if ("--restricted".equals(opt)) {
                newUserType = UserManager.USER_TYPE_FULL_RESTRICTED;
            } else if ("--guest".equals(opt)) {
                newUserType = UserManager.USER_TYPE_FULL_GUEST;
            } else if ("--demo".equals(opt)) {
                newUserType = UserManager.USER_TYPE_FULL_DEMO;
            } else if ("--ephemeral".equals(opt)) {
                flags |= UserInfo.FLAG_EPHEMERAL;
            } else if ("--for-testing".equals(opt)) {
                flags |= UserInfo.FLAG_FOR_TESTING;
            } else if ("--pre-create-only".equals(opt)) {
                preCreateOnly = true;
            } else if ("--user-type".equals(opt)) {
                newUserType = getNextArgRequired();
            } else {
                getErrPrintWriter().println("Error: unknown option " + opt);
                return 1;
            }
            // Ensure only one user-type was specified.
            if (newUserType != null) {
                if (userType != null && !userType.equals(newUserType)) {
                    getErrPrintWriter().println("Error: more than one user type was specified ("
                            + userType + " and " + newUserType + ")");
                    return 1;
                }
                userType = newUserType;
            }
        }
        String arg = getNextArg();
        if (arg == null && !preCreateOnly) {
            getErrPrintWriter().println("Error: no user name specified.");
            return 1;
        }
        if (arg != null && preCreateOnly) {
            getErrPrintWriter().println("Warning: name is ignored for pre-created users");
        }

        name = arg;
        UserInfo info = null;
        IUserManager um = com.android.server.pm.NativeUserManagerBridge.currentUserManagerOwner();
        IAccountManager accm = IAccountManager.Stub.asInterface(
                ServiceManager.getService("account"));
        if (userType == null) {
            userType = UserInfo.getDefaultUserType(flags);
        }
        Trace.traceBegin(Trace.TRACE_TAG_PACKAGE_MANAGER, "shell_runCreateUser");
        try {
            if (UserManager.isUserTypeRestricted(userType)) {
                // In non-split user mode, userId can only be SYSTEM
                int parentUserId = userId >= 0 ? userId : UserHandle.USER_SYSTEM;
                info = um.createRestrictedProfileWithThrow(name, parentUserId);
                accm.addSharedAccountsFromParentUser(parentUserId, userId,
                        (Process.myUid() == Process.ROOT_UID) ? "root" : "com.android.shell");
            } else if (userId < 0) {
                info = preCreateOnly ?
                        um.preCreateUserWithThrow(userType) :
                        um.createUserWithThrow(name, userType, flags);
            } else {
                info = um.createProfileForUserWithThrow(name, userType, flags, userId, null);
            }
        } catch (ServiceSpecificException e) {
            getErrPrintWriter().println("Error: " + e);
        } finally {
            Trace.traceEnd(Trace.TRACE_TAG_PACKAGE_MANAGER);
        }

        if (info != null) {
            getOutPrintWriter().println("Success: created user id " + info.id);
            return 0;
        } else {
            getErrPrintWriter().println("Error: couldn't create User.");
            return 1;
        }
    }

}
