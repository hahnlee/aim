package dev.darwinart.runtime.am;

import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.UserHandle;
import android.os.storage.IStorageManager;
import android.os.storage.StorageManager;
import com.android.server.LocalServices;
import com.android.server.SystemServiceManager;
import com.android.server.pm.UserManagerInternal;
import com.android.server.pm.UserManagerService;
import com.android.server.utils.TimingsTraceAndSlog;

/**
 * UserController's start of the system user (ActivityManagerService
 * systemReady and finishBooting): the SystemService user lifecycle and the
 * UserManager user state, from booting to running-unlocked. The system user
 * has no lock-screen credential here, so LockSettingsService's
 * unlockUserKeyIfUnsecured step unlocks its CE storage.
 */
public final class SystemUserLifecycle {
    private static final int USER = UserHandle.USER_SYSTEM;
    // com.android.server.am.UserState; R8 inlined the constants out of the
    // image's services.jar.
    private static final int STATE_RUNNING_LOCKED = 1;
    private static final int STATE_RUNNING_UNLOCKING = 2;
    private static final int STATE_RUNNING_UNLOCKED = 3;

    private SystemUserLifecycle() {}

    /** UserController.onSystemUserStarting (ActivityManagerService.systemReady). */
    public static void onSystemUserStarting(SystemServiceManager services) {
        services.onUserStarting(TimingsTraceAndSlog.newAsyncLog(), USER);
    }

    /**
     * UserController.finishUserBoot and the unlock that follows it
     * (ActivityManagerService.finishBooting): booting, running-locked,
     * running-unlocking, running-unlocked.
     */
    public static void finishUserBoot(SystemServiceManager services) throws RemoteException {
        UserManagerInternal users = LocalServices.getService(UserManagerInternal.class);
        users.setUserState(USER, STATE_RUNNING_LOCKED);
        IStorageManager.Stub.asInterface(ServiceManager.getService("mount"))
                .unlockCeStorage(USER, null);
        // finishUserUnlocking proceeds only once CE storage is unlocked.
        if (!StorageManager.isCeStorageUnlocked(USER)) {
            throw new IllegalStateException("system user CE storage did not unlock");
        }
        UserManagerService.getInstance().onBeforeUnlockUser(USER);
        users.setUserState(USER, STATE_RUNNING_UNLOCKING);
        services.onUserUnlocking(USER);
        // finishUserUnlocked
        users.setUserState(USER, STATE_RUNNING_UNLOCKED);
        services.onUserUnlocked(USER);
    }
}
