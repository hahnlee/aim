package dev.aim.server;

import static android.system.OsConstants.S_IRWXG;
import static android.system.OsConstants.S_IRWXO;

import android.app.ActivityThread;
import android.app.IApplicationThread;
import android.content.Context;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.UserHandle;
import android.os.UserManager;
import android.system.Os;
import android.util.Slog;

import com.android.internal.os.ApplicationSharedMemory;
import com.android.internal.os.ClassLoaderFactory;
import com.android.internal.os.RuntimeInit;
import com.android.safetycenter.SafetyCenterService;
import com.android.server.SystemService;
import com.android.server.SystemService.TargetUser;
import com.android.server.adservices.SystemProcessLifecycle;
import com.android.server.appsearch.AppSearchModule$Lifecycle;
import com.android.server.pinner.PinnerService;
import com.android.server.print.PrintManagerService;
import com.android.server.stats.pull.StatsPullAtomService;

import dalvik.system.VMRuntime;

import java.util.ArrayList;
import java.util.List;

/**
 * The app host's system process (docs/app-host.md, ADR 0013): a process of
 * the system uid, started by init with `app_process`, which runs what only
 * system_server runs as system_server runs it: the original AppSearch
 * module, the Connectivity module's NearbyService, the AdServices module's
 * AdServicesManagerService, DeviceLockService, TelecomLoaderService's
 * part ({@link TelecomLoader}), StatsCompanionService's UID-map/receiver
 * lifecycle ({@link StatsCompanionUidMap}), the original PinnerService,
 * PrintManagerService and StatsPullAtomService,
 * and the components declared in
 * `android:process="system"`, such as Telecom's.
 * This class plays ZygoteInit's part for the system server
 * (`handleSystemServerProcess`): its umask, its class loader of
 * SYSTEMSERVERCLASSPATH with a linker namespace (`createPathClassLoader`,
 * through which the module's JNI library loads), and the start of
 * {@link Server} in it as `zygoteInit` starts SystemServer, past
 * `commonInit`, which `app_process` has run (`applicationInit`,
 * RuntimeInit's to its subclasses).
 */
public final class SystemProcessHost extends RuntimeInit {
    private SystemProcessHost() {}

    public static void main(String[] args) {
        Os.umask(S_IRWXG | S_IRWXO);
        String libraryPath = System.getProperty("java.library.path");
        ClassLoader classLoader = ClassLoaderFactory.createClassLoader(
                System.getenv("SYSTEMSERVERCLASSPATH")
                        + ":/apex/com.android.permission/javalib/service-permission.jar",
                libraryPath, libraryPath,
                ClassLoader.getSystemClassLoader().getParent(),
                VMRuntime.SDK_VERSION_CUR_DEVELOPMENT, true /* isNamespaceShared */,
                null /* classLoaderName */);
        Thread.currentThread().setContextClassLoader(classLoader);
        applicationInit(VMRuntime.SDK_VERSION_CUR_DEVELOPMENT, null,
                new String[] {Server.class.getName()}, classLoader).run();
    }

    /**
     * SystemServer's and SystemServiceManager's part: the system context
     * (`createSystemContext`) and the mainline modules' initialization; the
     * system process's application thread handed to the app host, as
     * `setSystemProcess` makes it the system process's, which then installs
     * its providers (`installSystemProviders`) and runs its components on
     * this main thread; the services started in SystemServer's order and
     * given the boot phases from where SystemServer starts each: Pinner, then Nearby,
     * which ConnectivityServiceInitializer starts before
     * PHASE_LOCK_SETTINGS_READY (the app host serves the initializer's
     * other services natively), then PrintManagerService when FEATURE_PRINTING
     * is declared, then StatsPullAtomService and
     * AdServicesManagerService's Lifecycle ({@link SystemProcessLifecycle}),
     * then SafetyCenterService and AppSearch's Lifecycle
     * (after PHASE_DEVICE_SPECIFIC_SERVICES_READY), with user 0 started and
     * unlocked; DeviceLockService after AppSearch when FEATURE_DEVICE_LOCK
     * is declared; then TelecomLoaderService at PHASE_ACTIVITY_MANAGER_READY.
     * Nearby publishes `nearby`, AdServices `adservices_manager`, AppSearch
     * `app_search`, DeviceLock `device_lock`, Telecom `telecom`.
     */
    public static final class Server {
        private static final String TAG = "AimSystemProcess";
        /** SystemServer's CONNECTIVITY_SERVICE_APEX_PATH. */
        private static final String CONNECTIVITY_JAR =
                "/apex/com.android.tethering/javalib/service-connectivity.jar";
        /** Connectivity initializers for Nearby and cold netstats calls (java/nearby-initializer). */
        private static final String NEARBY_JAR = "/system/framework/aim-nearby.jar";
        private static final String NEARBY_INITIALIZER =
                "dev.aim.server.nearby.NearbyServiceInitializer";
        /** SystemServer's FEATURE_DEVICE_LOCK branch (android-16.0.0_r1). */
        private static final String DEVICE_LOCK_JAR =
                "/apex/com.android.devicelock/javalib/service-devicelock.jar";
        private static final String DEVICE_LOCK_SERVICE =
                "com.android.server.devicelock.DeviceLockService";

        private Server() {}

        public static void main(String[] args)
                throws ReflectiveOperationException, RemoteException,
                PackageManager.NameNotFoundException {
            Looper.prepareMainLooper();
            // SystemServer loads the original JNI registrations before its system context.
            System.loadLibrary("android_servers");
            ActivityThread thread = ActivityThread.systemMain();
            Context context = thread.getSystemContext();
            ActivityThread.initializeMainlineModules();
            // SystemServer.startBootstrapServices: required by DexUseManagerLocal's Injector.
            com.android.server.art.ArtModuleServiceInitializer.setArtModuleServiceManager(
                    new android.os.ArtModuleServiceManager());
            // SystemServer initializes mutable nonce storage after the modules and
            // before services: PlatformCompat invalidates IpcDataCache at construction.
            // Keep its mutable mapping and fd for the lifetime of this system process.
            ApplicationSharedMemory.setInstance(ApplicationSharedMemory.create());
            // ActivityManagerService.setSystemProcess: framework manifest
            // receivers use the system server's class loader, not the boot loader.
            thread.installSystemApplicationInfo(context.getPackageManager().getApplicationInfo(
                    "android", PackageManager.GET_SHARED_LIBRARY_FILES
                            | PackageManager.MATCH_SYSTEM_ONLY), Server.class.getClassLoader());
            IApplicationThread applicationThread = thread.getApplicationThread();
            ISystemProcess host = ISystemProcess.Stub.asInterface(
                    ServiceManager.waitForService("aim.system_process"));
            host.attachDexUseBridge(new com.android.server.art.AppHostDexUse(
                    context, host.getDexUsePackages()));
            host.attachNetworkActivityBridge(new NetworkActivityBridge(
                    context, new Handler(Looper.getMainLooper()), host));
            host.attachActivityAnimations(new ActivityAnimations(context));
            host.attachWifiBridge(new WifiBridge(new Handler(Looper.getMainLooper())));
            host.attachDevicePolicyDefaults(new DevicePolicyDefaults(context));
            ServiceManager.addService("aim.role_queries", new RoleQueries(context));
            host.attachSettingsValues(new SettingsValues());
            host.attachProviderQueries(new ProviderQueries(context));
            var installer = new com.android.server.pm.Installer(context);
            installer.onStart();
            var shutdown = PackageLocalOwners.publish(host, context, installer);
            var packageBridge = new PackageBootstrapBridge(null, installer);
            packageBridge.installPackageShutdownOwner(shutdown);
            host.attachPackageBootstrapJava(packageBridge);
            host.attachNativeStagingBridge(new com.android.server.pm.NativeStagingBridge());
            host.attachAudioPolicyQueries(new AudioPolicyQueries(context));
            host.initializeCameraPrivacyAllowlist(new ArrayList<>(
                    com.android.server.SystemConfig.getInstance().getCameraPrivacyAllowlist()));
            host.attachUserSwitcherPolicy(new UserSwitcherPolicy(context));
            host.attachPackageIntentParser(new PackageIntentParser());
            host.attachPackageShellRead(new AppHostPackageShellRead(context));
            host.attachDomainVerification(new AppHostDomainVerification(context, host));
            PreBootNotifications.publish(context);
            // Publish the original HWUI buffer owner before apps can attach.
            ServiceManager.addService(android.graphics.GraphicsStatsService.GRAPHICS_STATS_SERVICE,
                    new android.graphics.GraphicsStatsService(context));
            host.attachSystemProcess(applicationThread.asBinder(),
                    DeviceServices.foregroundServicePolicy(context), new SafeLabels(),
                    new BatteryHealthStats(context));
            MediaArtwork.publish(context);
            AlarmIndicator.publish(context);
            host.attachSystemRouteQueries(new com.android.server.media.SystemRouteQueries(context));
            host.attachStatsCompanionUidMap(new StatsCompanionUidMap(context));
            host.attachRecoverableKeyStoreKeys(new RecoverableKeyStoreKeys(context));
            host.attachStorageCrates(new StorageCrates(context));
            ActivityStats.publish(host);
            NativeJobScheduler.publish(host);
            SyncPolicyLocals.publish(host);
            SyncOwnerStartup.start(context, host);
            TaskManagerLocal.publish(host);
            AccessibilityWindows.publish(context, host);
            com.android.server.LocalManagerRegistry.addManager(
                    com.android.server.role.RoleServicePlatformHelper.class,
                    new com.android.server.policy.role.RoleServicePlatformHelperImpl(context));
            SystemService role = new com.android.role.RoleService(context);
            role.onStart();
            host.attachRoleService(ServiceManager.getService(Context.ROLE_SERVICE));
            SystemService accessibility = new com.android.server.accessibility.AccessibilityManagerService$Lifecycle(context);
            accessibility.onStart();
            SystemService pinner = new PinnerService(context);
            pinner.onStart();
            // ZygoteInit's class loader of the standalone jar
            // (`prefetchStandaloneSystemServerJars`, as
            // SystemServerClassLoaderFactory makes it), and the stand-in's
            // below it.
            ClassLoader connectivity = ClassLoaderFactory.createClassLoader(CONNECTIVITY_JAR,
                    null, null, Server.class.getClassLoader(), Build.VERSION.SDK_INT,
                    true /* isNamespaceShared */, null /* classLoaderName */);
            ClassLoader nearbyLoader = ClassLoaderFactory.createClassLoader(NEARBY_JAR, null,
                    null, connectivity, Build.VERSION.SDK_INT, true /* isNamespaceShared */,
                    null /* classLoaderName */);
            startService("dev.aim.server.nearby.ConnectivityDumpInitializer", nearbyLoader, context);
            startService("dev.aim.server.nearby.NetworkStatsInitializer", nearbyLoader, context);
            SystemService nearby = startService(NEARBY_INITIALIZER, nearbyLoader, context);
            List<SystemService> services = new ArrayList<>(List.of(pinner, nearby, accessibility, role));
            if (context.getPackageManager().hasSystemFeature(PackageManager.FEATURE_PRINTING)) {
                SystemService print = new PrintManagerService(context);
                print.onStart();
                services.add(print);
            }
            SystemService statsPull = new StatsPullAtomService(context);
            statsPull.onStart();
            SystemService adServices = SystemProcessLifecycle.start(context);
            services.add(statsPull);
            services.add(adServices);
            for (int phase : new int[] {SystemService.PHASE_LOCK_SETTINGS_READY,
                    SystemService.PHASE_SYSTEM_SERVICES_READY,
                    SystemService.PHASE_DEVICE_SPECIFIC_SERVICES_READY}) {
                for (SystemService service : services) {
                    service.onBootPhase(phase);
                }
            }
            SystemService safetyCenter = new SafetyCenterService(context);
            safetyCenter.onStart();
            SystemService appSearch = new AppSearchModule$Lifecycle(context);
            appSearch.onStart();
            services.add(safetyCenter);
            services.add(appSearch);
            if (context.getPackageManager().hasSystemFeature(PackageManager.FEATURE_DEVICE_LOCK)) {
                ClassLoader deviceLockLoader = ClassLoaderFactory.createClassLoader(DEVICE_LOCK_JAR,
                        null, null, Server.class.getClassLoader(), Build.VERSION.SDK_INT,
                        true /* isNamespaceShared */, null /* classLoaderName */);
                services.add(startService(DEVICE_LOCK_SERVICE, deviceLockLoader, context));
            }
            for (int phase : new int[] {SystemService.PHASE_ACTIVITY_MANAGER_READY,
                    SystemService.PHASE_THIRD_PARTY_APPS_CAN_START}) {
                for (SystemService service : services) {
                    service.onBootPhase(phase);
                }
            }
            // The app host runs user 0 alone, unlocked from its start.
            TargetUser user = new TargetUser(context.getSystemService(UserManager.class)
                    .getUserInfo(UserHandle.USER_SYSTEM));
            for (SystemService service : services) {
                service.onUserStarting(user);
            }
            pinner.onUserSwitching(null, user);
            accessibility.onUserSwitching(null, user);
            for (SystemService service : services) {
                service.onUserUnlocking(user);
            }
            for (SystemService service : services) {
                service.onUserUnlocked(user);
            }
            for (SystemService service : services) {
                service.onBootPhase(SystemService.PHASE_BOOT_COMPLETED);
            }
            // This image initializes accessibility from USER_SWITCHED (its Lifecycle flag is off).
            // Ordered completion publishes the optional bindApplication service-cache entry.
            context.sendOrderedBroadcastAsUser(new android.content.Intent(android.content.Intent.ACTION_USER_SWITCHED)
                    .putExtra(android.content.Intent.EXTRA_USER_HANDLE, UserHandle.USER_SYSTEM)
                    .addFlags(android.content.Intent.FLAG_RECEIVER_REGISTERED_ONLY | android.content.Intent.FLAG_RECEIVER_FOREGROUND),
                    UserHandle.ALL, null, new android.content.BroadcastReceiver() {
                        @Override public void onReceive(Context c, android.content.Intent intent) {
                            try { host.accessibilityReady(ServiceManager.getService(Context.ACCESSIBILITY_SERVICE)); }
                            catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
                        }
                    }, new Handler(Looper.getMainLooper()), 0, null, null);
            Slog.i(TAG, "System process services started");
            new TelecomLoader(context).onActivityManagerReady();
            Looper.loop();
        }

        /**
         * SystemServiceManager's `startServiceFromJar`: the class SystemServer
         * names, loaded from the jar's class loader (`loadClassFromLoader`),
         * constructed with the context and started (`startService`).
         */
        private static SystemService startService(String className, ClassLoader classLoader,
                Context context) throws ReflectiveOperationException {
            SystemService service = Class.forName(className, true, classLoader)
                    .asSubclass(SystemService.class)
                    .getConstructor(Context.class)
                    .newInstance(context);
            service.onStart();
            return service;
        }
    }
}
