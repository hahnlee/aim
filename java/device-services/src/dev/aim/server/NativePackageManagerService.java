package dev.aim.server;

import android.content.Context;
import android.content.pm.PackageManagerInternal;
import android.os.Binder;
import android.os.IBinder;
import android.os.Process;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.util.DisplayMetrics;
import com.android.server.LocalManagerRegistry;
import com.android.server.LocalServices;
import com.android.server.pm.Computer;
import com.android.server.pm.Installer;
import com.android.server.pm.NativeComputer;
import com.android.server.pm.NativeUserManagerBridge;
import com.android.server.pm.PackageManagerLocal;
import com.android.server.pm.PackageManagerService;
import com.android.server.pm.PackageManagerTracedLock;
import com.android.server.pm.UserManagerService;
import com.android.server.pm.verify.domain.DomainVerificationService;
import java.util.Objects;
import java.util.concurrent.Future;
import java.util.function.Function;

/** Inactive D1 entry points. The concrete PMS receiver slot is deliberately null. */
public final class NativePackageManagerService {
    private NativePackageManagerService() {}

    /** Actual native constructor begins before the original UM dependency is created. */
    public interface BootstrapFactory {
        Construction begin(Context context, Installer installer,
                DomainVerificationService domains, boolean factoryTest, IServiceHost host)
                throws RemoteException;
    }
    public interface Construction {
        PackageManagerTracedLock installLock();
        Object packageLock();
        NativeUserManagerBridge.Owner userPackages();
        /** Scan/admit/persist native state and construct the complete facade owners. */
        Inputs finish(UserManagerService users) throws RemoteException;
        void abort(Throwable failure);
    }
    public interface Lifecycle {
        boolean isFirstBoot();
        boolean isDeviceUpgrading();
        Future<?> appDataPreparation();
        void updatePackagesIfNeeded();
        void updateMetrics(DisplayMetrics metrics);
        void performFstrimIfNeeded();
        void prepareSystemReady();
        void registerStorageListeners();
        void installerSystemReady();
        void dexOptimizerSystemReady();
        void reconcileInternalApps();
        java.util.List<android.content.pm.UserInfo> livingUsers();
        void registerPackageObservers();
        void moduleInfoSystemReady();
        void restoreAndApplyStagedSessions();
        void schedulePostReadyMaintenance();
        void shutdown();
    }
    public interface ArtOwner {
        /** Original ArtManagerLocal over the published native PackageManagerLocal. */
        void initialize(Context context, PackageManagerLocal packages);
    }
    /** Real original ART Service startup without a concrete PMS dependency. */
    public static final class OriginalArtOwner implements ArtOwner {
        private final com.android.server.art.ArtManagerLocal.DexoptDoneCallback done;
        public OriginalArtOwner(com.android.server.art.ArtManagerLocal.DexoptDoneCallback done) {
            this.done = Objects.requireNonNull(done);
        }
        @Override public void initialize(Context context, PackageManagerLocal packages) {
            if (LocalManagerRegistry.getManager(PackageManagerLocal.class) != packages)
                throw new IllegalStateException("ART PackageManagerLocal generation differs");
            if (LocalManagerRegistry.getManager(com.android.server.art.ArtManagerLocal.class) != null)
                throw new IllegalStateException("ART manager already registered");
            var art = new com.android.server.art.ArtManagerLocal(context);
            art.addDexoptDoneCallback(false, Runnable::run, done);
            LocalManagerRegistry.addManager(com.android.server.art.ArtManagerLocal.class, art);
            context.registerReceiver(new android.content.BroadcastReceiver() {
                @Override public void onReceive(Context received, android.content.Intent intent) {
                    received.unregisterReceiver(this);
                    art.scheduleBackgroundDexoptJob();
                }
            }, new android.content.IntentFilter(android.content.Intent.ACTION_LOCKED_BOOT_COMPLETED));
            var nativePackages = android.content.pm.IPackageManagerNative.Stub.asInterface(
                    Objects.requireNonNull(ServiceManager.getService("package_native")));
            try {
                nativePackages.registerStagedApexObserver(new android.content.pm.IStagedApexObserver.Stub() {
                    @Override public void onApexStaged(android.content.pm.ApexStagedEvent event) {
                        art.onApexStaged(java.util.Arrays.stream(event.stagedApexInfos)
                                .map(info -> info.moduleName).toArray(String[]::new));
                    }
                });
            } catch (RemoteException failure) {
                throw new IllegalStateException("native staged ART observer unavailable", failure);
            }
        }
    }
    public record Inputs(PackageSnapshots.Store snapshots,
            NativePackageManagerInternal internal, PackageLocal local,
            Function<PackageSnapshots.ComputerSnapshot, NativeComputer> computers,
            Lifecycle lifecycle, ArtOwner art, PackagePermissionLifecycle permissions,
            IBinder packageService, IBinder nativeService) {
        public Inputs {
            Objects.requireNonNull(snapshots); Objects.requireNonNull(internal);
            Objects.requireNonNull(local); Objects.requireNonNull(computers);
            Objects.requireNonNull(lifecycle); Objects.requireNonNull(art); Objects.requireNonNull(permissions);
            Objects.requireNonNull(packageService); Objects.requireNonNull(nativeService);
        }
    }
    private static BootstrapFactory factory;
    private static Coordinator current;
    /** NativeBootstrap installs its concrete factory before the D1 main redirect. */
    public static synchronized void configure(BootstrapFactory owner) {
        if (factory != null || current != null) throw new IllegalStateException("native package factory already bound");
        factory = Objects.requireNonNull(owner);
    }
    private static final class Coordinator {
        final Context context;
        final UserManagerService users;
        final Inputs owners;
        private boolean artInitialized;
        private boolean ready;
        private volatile boolean closed;
        Coordinator(Context context, UserManagerService users, Inputs owners) {
            this.context = context; this.users = users; this.owners = owners;
        }
        void publish() throws RemoteException {
            owners.snapshots().getVersion();
            if (ServiceManager.checkService("package") != owners.packageService()
                    || ServiceManager.checkService("package_native") != owners.nativeService()) {
                throw new IllegalStateException("native endpoints are not registered under their original names");
            }
            if (!"android.content.pm.IPackageManager".equals(owners.packageService().getInterfaceDescriptor())
                    || !"android.content.pm.IPackageManagerNative".equals(owners.nativeService().getInterfaceDescriptor())) {
                throw new IllegalStateException("native package endpoint descriptors differ");
            }
            if (LocalServices.getService(PackageManagerInternal.class) != owners.internal()
                    || LocalManagerRegistry.getManager(PackageManagerLocal.class) != owners.local())
                throw new IllegalStateException("prepared package local owner instances differ");
        }
        synchronized void initializeArt(Context supplied) {
            if (supplied != context) throw new IllegalArgumentException("foreign ART constructor context");
            if (artInitialized) throw new IllegalStateException("ART owner initialized twice");
            owners.art().initialize(context, owners.local());
            artInitialized = true;
        }
        synchronized void ready() {
            if (ready) throw new IllegalStateException("package owner ready twice");
            if (!artInitialized) throw new IllegalStateException("ART local owner was not initialized");
            Lifecycle lifecycle = owners.lifecycle();
            lifecycle.prepareSystemReady();
            NativeUserManagerBridge.systemReady(users);
            lifecycle.registerStorageListeners();
            lifecycle.installerSystemReady();
            lifecycle.dexOptimizerSystemReady();
            NativeUserManagerBridge.reconcileUsers(users, android.os.storage.StorageManager.UUID_PRIVATE_INTERNAL);
            lifecycle.reconcileInternalApps();
            owners.permissions().systemReady(Objects.requireNonNull(lifecycle.livingUsers()));
            lifecycle.registerPackageObservers();
            lifecycle.moduleInfoSystemReady();
            lifecycle.restoreAndApplyStagedSessions();
            lifecycle.schedulePostReadyMaintenance();
            ready = true;
        }
    }
    private static void enforceSystem() {
        int uid = Binder.getCallingUid();
        if (uid != Process.SYSTEM_UID && uid != Process.ROOT_UID) throw new SecurityException("native package lifecycle requires system or root");
    }
    private static synchronized Coordinator owner(PackageManagerService receiver) {
        if (receiver != null) throw new IllegalStateException("D1 received an original PMS instance");
        if (current == null || current.closed) throw new IllegalStateException("native package constructor is unavailable or closed");
        return current;
    }
    public static synchronized PackageManagerService main(Context context, Installer installer,
            DomainVerificationService domains, boolean factoryTest) {
        enforceSystem(); Objects.requireNonNull(context); Objects.requireNonNull(installer);
        Objects.requireNonNull(domains);
        if (current != null) throw new IllegalStateException("native package constructor called twice");
        if (!android.os.SystemProperties.getBoolean("config.disable_otadexopt", false))
            throw new IllegalStateException("D1 requires the image's disabled OTA dexopt call site");
        BootstrapFactory bootstrap = factory;
        if (bootstrap == null) bootstrap = NativePackageBootstrap::production;
        IBinder hostBinder = Objects.requireNonNull(ServiceManager.getService("aim.service_host"), "native service host unavailable");
        Construction construction = null;
        try {
            construction = Objects.requireNonNull(bootstrap.begin(context, installer, domains,
                    factoryTest, IServiceHost.Stub.asInterface(hostBinder)));
            UserManagerService users = NativeUserManagerBridge.create(context, installer,
                    construction.installLock(), construction.packageLock(), construction.userPackages());
            if (ServiceManager.checkService("user") != null)
                throw new IllegalStateException("original user Binder already published");
            ServiceManager.addService("user", users);
            if (android.os.UserManager.isHeadlessSystemUserMode())
                throw new IllegalStateException("D1 headless initializer requires its concrete owner redirect");
            Inputs inputs = Objects.requireNonNull(construction.finish(users));
            Coordinator next = new Coordinator(context, users, inputs);
            next.publish();
            hostBinder.linkToDeath(() -> {
                synchronized (NativePackageManagerService.class) {
                    next.closed = true;
                    if (current == next) current = null;
                }
                try { next.owners.lifecycle().shutdown(); }
                finally { next.owners.snapshots().close(); }
            }, 0);
            if (next.closed) throw new IllegalStateException("native service host closed during construction");
            current = next;
            return null;
        } catch (RemoteException | RuntimeException | Error failure) {
            if (construction != null) {
                try { construction.abort(failure); }
                catch (RuntimeException cleanup) { failure.addSuppressed(cleanup); }
            }
            throw new IllegalStateException("native PackageManager construction failed", failure);
        }
    }
    public static boolean isFirstBoot(PackageManagerService receiver) { return owner(receiver).owners.lifecycle().isFirstBoot(); }
    public static boolean isDeviceUpgrading(PackageManagerService receiver) { return owner(receiver).owners.lifecycle().isDeviceUpgrading(); }
    public static void waitForAppDataPrepared(PackageManagerService receiver) {
        enforceSystem(); Future<?> prepared = owner(receiver).owners.lifecycle().appDataPreparation();
        if (prepared == null) throw new IllegalStateException("native app-data barrier unavailable");
        com.android.internal.util.ConcurrentUtils.waitForFutureNoInterrupt(prepared, "wait for native prepareAppData");
    }
    public static void initializeArtManagerLocal(Context context, PackageManagerService receiver) { enforceSystem(); owner(receiver).initializeArt(context); }
    public static void updatePackagesIfNeeded(PackageManagerService receiver) { enforceSystem(); owner(receiver).owners.lifecycle().updatePackagesIfNeeded(); }
    public static void updateMetricsIfNeeded(PackageManagerService receiver) {
        enforceSystem(); Coordinator coordinator = owner(receiver);
        android.hardware.display.DisplayManager manager = coordinator.context.getSystemService(android.hardware.display.DisplayManager.class);
        if (manager != null) {
            android.view.Display display = manager.getDisplay(android.view.Display.DEFAULT_DISPLAY);
            if (display != null) { DisplayMetrics metrics = new DisplayMetrics(); display.getMetrics(metrics); coordinator.owners.lifecycle().updateMetrics(metrics); }
        }
    }
    public static void performFstrimIfNeeded(PackageManagerService receiver) { enforceSystem(); owner(receiver).owners.lifecycle().performFstrimIfNeeded(); }
    public static void systemReady(PackageManagerService receiver) { enforceSystem(); owner(receiver).ready(); }
    public static Computer snapshotComputer(PackageManagerService receiver) {
        Coordinator coordinator = owner(receiver);
        return Objects.requireNonNull(coordinator.owners.computers().apply(coordinator.owners.snapshots().computer()));
    }
}
