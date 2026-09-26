package dev.darwinart.runtime.system;

import android.app.ActivityThread;
import android.app.IApplicationThread;
import android.content.Context;
import android.os.ArtModuleServiceManager;
import android.os.Process;
import android.os.ServiceManager;
import android.os.SystemClock;
import com.android.server.LocalManagerRegistry;
import com.android.server.LocalServices;
import com.android.server.SystemConfig;
import com.android.server.SystemConfigService;
import com.android.server.SystemService;
import com.android.server.ServiceThread;
import com.android.server.SystemServerInitThreadPool;
import com.android.server.SystemServiceManager;
import com.android.server.utils.TimingsTraceAndSlog;
import com.android.server.apphibernation.AppHibernationService;
import com.android.server.appop.AppOpMigrationHelper;
import com.android.server.appop.AppOpsService;
import com.android.server.art.ArtModuleServiceInitializer;
import com.android.server.art.DexUseManagerLocal;
import com.android.server.appop.AppOpMigrationHelperImpl;
import com.android.server.compat.PlatformCompat;
import com.android.server.compat.PlatformCompatNative;
import com.android.server.pm.DexOptHelper;
import com.android.server.pm.Installer;
import com.android.server.pm.PackageManagerService;
import com.android.server.pm.UserManagerService;
import com.android.server.pm.permission.PermissionMigrationHelper;
import com.android.server.pm.permission.PermissionMigrationHelperImpl;
import com.android.server.permission.access.AccessCheckingService;
import com.android.server.pm.verify.domain.DomainVerificationService;
import com.android.server.sensorprivacy.SensorPrivacyService;
import dev.darwinart.runtime.am.ActivityManagerEndpoint;
import dev.darwinart.runtime.am.SystemUserLifecycle;
import dev.darwinart.runtime.pm.apex.DarwinApexService;
import dev.darwinart.runtime.pm.art.DarwinArtd;
import dev.darwinart.runtime.pm.installd.DarwinInstalld;
import java.io.File;

/**
 * The part of AOSP {@code SystemServer.startBootstrapServices} this runtime
 * runs, in the same order, with the original service owners. Activity,
 * window and the other system services are still this runtime's endpoints;
 * they are published by {@link SystemServiceFactory}.
 */
public final class SystemServerBootstrap {
    private static final String TAG = "SystemServerBootstrap";
    private SystemServerBootstrap() {}

    public static void startBootstrapServices(Context systemContext) throws Exception {
        // SystemServer.run, after createSystemContext: the mainline modules'
        // service managers (stats, telephony, media, ...).
        ActivityThread.initializeMainlineModules();
        // Before DexUseManagerLocal: links service-art.jar ahead of the
        // allocation-heavy PackageManagerService construction.
        ArtModuleServiceInitializer.setArtModuleServiceManager(new ArtModuleServiceManager());
        // SystemServer.run starts the init pool before the bootstrap services.
        SystemServerInitThreadPool.start();
        // Native daemons that start before system_server on Android; here
        // binders in this process (ADR 0009).
        ServiceManager.addService(DarwinApexService.SERVICE_NAME, new DarwinApexService(
                new File("/apex/apex-info-list.xml"), new File("/apex")));
        ServiceManager.addService(DarwinInstalld.SERVICE_NAME, new DarwinInstalld());
        ServiceManager.addService(DarwinArtd.SERVICE_NAME, new DarwinArtd());

        SystemServiceManager services = new SystemServiceManager(systemContext);
        services.setStartInfo(false, SystemClock.elapsedRealtime(), SystemClock.uptimeMillis());
        LocalServices.addService(SystemServiceManager.class, services);

        PlatformCompat platformCompat = new PlatformCompat(systemContext);
        ServiceManager.addService(Context.PLATFORM_COMPAT_SERVICE, platformCompat);
        ServiceManager.addService(Context.PLATFORM_COMPAT_NATIVE_SERVICE,
                new PlatformCompatNative(platformCompat));

        Installer installer = (Installer) services.startService(Installer.class);

        LocalServices.addService(PermissionMigrationHelper.class,
                new PermissionMigrationHelperImpl());
        LocalServices.addService(AppOpMigrationHelper.class, new AppOpMigrationHelperImpl());
        services.startService(AccessCheckingService.class);
        // ActivityManagerService's constructor and start(): the AppOps owner.
        File systemDir = SystemServiceManager.ensureSystemDir();
        ServiceThread appOpsThread =
                new ServiceThread("AppOps", Process.THREAD_PRIORITY_FOREGROUND, false);
        appOpsThread.start();
        AppOpsService appOps = new AppOpsService(new File(systemDir, "appops_accesses.xml"),
                new File(systemDir, "appops.xml"), appOpsThread.getThreadHandler(),
                systemContext);
        appOps.publish();
        TimingsTraceAndSlog t = new TimingsTraceAndSlog();
        services.startBootPhase(t, SystemService.PHASE_WAIT_FOR_DEFAULT_DISPLAY);

        DomainVerificationService domainVerification = new DomainVerificationService(
                systemContext, SystemConfig.getInstance(), platformCompat);
        services.startService(domainVerification);
        PackageManagerService packageManager =
                PackageManagerService.main(systemContext, installer, domainVerification, false);
        // After PackageManagerLocal is registered, before PMS serves
        // notifyDexLoad.
        LocalManagerRegistry.addManager(
                DexUseManagerLocal.class, DexUseManagerLocal.createInstance(systemContext));
        services.startService(UserManagerService.LifeCycle.class);
        // ActivityManagerService.setSystemProcess
        IApplicationThread systemThread =
                ActivityThread.currentActivityThread().getApplicationThread();
        ((ActivityManagerEndpoint) ServiceManager.getService(Context.ACTIVITY_SERVICE))
                .setSystemProcess(systemThread.asBinder());
        services.startService(new SensorPrivacyService(systemContext));
        android.util.Slog.i(TAG, "Bootstrap services started");
        // SystemServer.startCoreServices
        services.startService(SystemConfigService.class);

        // SystemServer.startOtherServices, for the services started above.
        services.startService(AppHibernationService.class);
        DexOptHelper.initializeArtManagerLocal(systemContext, packageManager);
        services.startServiceFromJar("com.android.server.stats.StatsCompanion$Lifecycle",
                "/apex/com.android.os.statsd/javalib/service-statsd.jar");
        services.startBootPhase(t, SystemService.PHASE_LOCK_SETTINGS_READY);
        services.startBootPhase(t, SystemService.PHASE_SYSTEM_SERVICES_READY);
        packageManager.systemReady();
        services.startBootPhase(t, SystemService.PHASE_DEVICE_SPECIFIC_SERVICES_READY);
        // ActivityManagerService.systemReady
        services.preSystemReady();
        appOps.systemReady();
        SystemUserLifecycle.onSystemUserStarting(services);
        services.startBootPhase(t, SystemService.PHASE_ACTIVITY_MANAGER_READY);
        packageManager.waitForAppDataPrepared();
        services.startBootPhase(t, SystemService.PHASE_THIRD_PARTY_APPS_CAN_START);
        // ActivityManagerService.finishBooting; this phase also retires the
        // init thread pool (SystemServiceManager.shutdownInitThreadPool).
        services.startBootPhase(t, SystemService.PHASE_BOOT_COMPLETED);
        // "Tell anyone interested that we are done booting!"
        android.os.SystemProperties.set("sys.boot_completed", "1");
        android.os.SystemProperties.set("dev.bootcomplete", "1");
        SystemUserLifecycle.finishUserBoot(services);
        android.util.Slog.i(TAG, "System services ready");
    }
}
