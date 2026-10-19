package dev.aim.server;

import static android.system.OsConstants.S_IRWXG;
import static android.system.OsConstants.S_IRWXO;

import android.app.ActivityThread;
import android.content.Context;
import android.os.Looper;
import android.os.UserHandle;
import android.os.UserManager;
import android.system.Os;
import android.util.Slog;

import com.android.internal.os.ClassLoaderFactory;
import com.android.internal.os.RuntimeInit;
import com.android.server.SystemService;
import com.android.server.SystemService.TargetUser;
import com.android.server.appsearch.AppSearchModule$Lifecycle;

import dalvik.system.VMRuntime;

/**
 * The app host's AppSearch (docs/app-host.md, ADR 0013): a process of the
 * system uid, started by init with `app_process`, in which the original
 * AppSearch module runs as system_server runs it. This class plays
 * ZygoteInit's part for the system server (`handleSystemServerProcess`):
 * its umask, its class loader of SYSTEMSERVERCLASSPATH with a linker
 * namespace (`createPathClassLoader`, through which the module's JNI
 * library loads), and the start of {@link Server} in it as `zygoteInit`
 * starts SystemServer, past `commonInit`, which `app_process` has run
 * (`applicationInit`, RuntimeInit's to its subclasses).
 */
public final class AppSearchHost extends RuntimeInit {
    private AppSearchHost() {}

    public static void main(String[] args) {
        Os.umask(S_IRWXG | S_IRWXO);
        String libraryPath = System.getProperty("java.library.path");
        ClassLoader classLoader = ClassLoaderFactory.createClassLoader(
                System.getenv("SYSTEMSERVERCLASSPATH"), libraryPath, libraryPath,
                ClassLoader.getSystemClassLoader().getParent(),
                VMRuntime.SDK_VERSION_CUR_DEVELOPMENT, true /* isNamespaceShared */,
                null /* classLoaderName */);
        Thread.currentThread().setContextClassLoader(classLoader);
        applicationInit(VMRuntime.SDK_VERSION_CUR_DEVELOPMENT, null,
                new String[] {Server.class.getName()}, classLoader).run();
    }

    /**
     * SystemServer's and SystemServiceManager's part for the module: the
     * system context (`createSystemContext`) and the mainline modules'
     * initialization, the module's Lifecycle started where SystemServer
     * starts it (after PHASE_DEVICE_SPECIFIC_SERVICES_READY) and so given
     * the later boot phases, and user 0 started and unlocked. The module
     * publishes `app_search` itself.
     */
    public static final class Server {
        private static final String TAG = "AimAppSearchHost";

        private Server() {}

        public static void main(String[] args) {
            Looper.prepareMainLooper();
            Context context = ActivityThread.systemMain().getSystemContext();
            ActivityThread.initializeMainlineModules();
            SystemService appSearch = new AppSearchModule$Lifecycle(context);
            appSearch.onStart();
            appSearch.onBootPhase(SystemService.PHASE_ACTIVITY_MANAGER_READY);
            appSearch.onBootPhase(SystemService.PHASE_THIRD_PARTY_APPS_CAN_START);
            // The app host runs user 0 alone, unlocked from its start.
            TargetUser user = new TargetUser(context.getSystemService(UserManager.class)
                    .getUserInfo(UserHandle.USER_SYSTEM));
            appSearch.onUserStarting(user);
            appSearch.onUserUnlocking(user);
            appSearch.onUserUnlocked(user);
            appSearch.onBootPhase(SystemService.PHASE_BOOT_COMPLETED);
            Slog.i(TAG, "AppSearch started");
            Looper.loop();
        }
    }
}
