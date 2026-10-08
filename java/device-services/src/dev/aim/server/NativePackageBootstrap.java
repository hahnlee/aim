package dev.aim.server;

import android.content.Context;
import android.content.pm.ActivityInfo;
import android.os.Parcel;
import android.os.RemoteException;
import com.android.internal.os.BackgroundThread;
import com.android.server.pm.Computer;
import com.android.server.pm.Installer;
import com.android.server.pm.NativeComputer;
import com.android.server.pm.NativePackageUserOwner;
import com.android.server.pm.PackageArchiver;
import com.android.server.pm.PackageManagerTracedLock;
import com.android.server.pm.UserManagerService;
import com.android.server.pm.permission.LegacyPermissionManagerInternal;
import com.android.server.pm.permission.PermissionManagerServiceInternal;
import com.android.server.pm.verify.domain.DomainVerificationService;
import java.io.IOException;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.Future;
import java.util.concurrent.FutureTask;
import java.util.function.Function;

/** Concrete C assembly: original typed helpers over one native boot-session lifetime. */
public final class NativePackageBootstrap implements NativePackageManagerService.BootstrapFactory {
    @FunctionalInterface public interface ArchiverFactory {
        PackageArchiver create(PackageSnapshots.Store packages,IPackageInternalHost host,java.util.function.Supplier<NativeComputer> computers);
    }
    @FunctionalInterface public interface ResolverFactory {
        ResolverOwner create(Context context,PackageSnapshots.Store packages);
    }
    public record ResolverOwner(ActivityInfo activity,boolean replaced) {
        public ResolverOwner {Objects.requireNonNull(activity);}
    }
    /** Actual original instances whose concrete constructors remain system-server owners. */
    public record OriginalOwners(java.util.function.Supplier<PermissionManagerServiceInternal> permissions,
            java.util.function.Supplier<LegacyPermissionManagerInternal> legacy,
            ArchiverFactory archiver,
            ResolverFactory resolver,
            android.content.pm.dex.IArtManager artManager,
            com.android.server.pm.CompilerStats compilerStats,
            com.android.server.pm.dex.DexManager dexManager,
            com.android.server.pm.dex.DynamicCodeLogger dynamicCode,
            java.io.File parserCache,
            com.android.server.pm.NativePackageEffectsBridge.Policy effectsPolicy) {
        public OriginalOwners {
            Objects.requireNonNull(permissions); Objects.requireNonNull(legacy);
            Objects.requireNonNull(archiver);
            Objects.requireNonNull(resolver);
            Objects.requireNonNull(artManager); Objects.requireNonNull(compilerStats);
            Objects.requireNonNull(dexManager); Objects.requireNonNull(dynamicCode);
            Objects.requireNonNull(effectsPolicy);
        }
    }
    private final OriginalOwners originals;
    private final PackageManagerTracedLock sharedInstallLock;
    public NativePackageBootstrap(OriginalOwners originals,PackageManagerTracedLock sharedInstallLock) {
        this.originals=Objects.requireNonNull(originals);this.sharedInstallLock=Objects.requireNonNull(sharedInstallLock);
    }
    public static NativePackageManagerService.Construction production(Context context,Installer installer,
            DomainVerificationService domains,boolean factoryTest,IServiceHost host) throws RemoteException {
        var lock=new PackageManagerTracedLock();
        var owners=new com.android.server.pm.NativePackageOriginalOwners(context,installer,lock);
        var originals=new OriginalOwners(owners::permissions,owners::legacy,
                (packages,internal,computers)->new com.android.server.pm.NativePackageArchiver(context,packages,internal,computers,BackgroundThread.getHandler()),
                (actualContext,packages)->com.android.server.pm.NativePlatformResolver.create(actualContext,packages),
                owners.art(),owners.compiler(),owners.dex(),owners.dynamic(),owners.parserCache(),owners.effectsPolicy());
        return new NativePackageBootstrap(originals,lock).begin(context,installer,domains,factoryTest,host);
    }
    @Override public NativePackageManagerService.Construction begin(Context context, Installer installer,
            DomainVerificationService domains, boolean factoryTest, IServiceHost host) throws RemoteException {
        // This attaches only the typed policy/filesystem leaves; it never calls original PMS.main.
        NativePackageCapabilities.attach(host);
        PackageBootstrapBridge bridge = PackageBootstrapBridge.attachOwner(domains, installer);
        IPackageBootSession boot = NativePackageCapabilities.attach(host.beginPackageManagerBoot(factoryTest));
        try { return new Construction(context, installer, host, boot, bridge, originals, sharedInstallLock); }
        catch (RemoteException | RuntimeException failure) {
            try { boot.close(); } catch (RemoteException | RuntimeException cleanup) { failure.addSuppressed(cleanup); }
            throw failure;
        }
    }
    private static final class NativeSnapshotOwner implements PackageSnapshots.Owner {
        private final IPackageBootSession boot;
        NativeSnapshotOwner(IPackageBootSession boot) { this.boot = boot; }
        @Override public String getFilteredPackageName(long version, String name, int uid, int user) {
            try { return boot.filterPackageName(version, name, uid, user); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }
        @Override public boolean shouldFilter(long version, com.android.server.pm.pkg.PackageState state, int uid, int user) {
            try { return boot.shouldFilter(version, state.getPackageName(), uid, user); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }
    }
    private record State(boolean firstBoot, boolean upgrading, boolean crossUser,boolean safeMode,String platformPackage,
            Map<Integer,Boolean> webDisabled) {
        static State read(byte[] bytes) {
            Parcel record = Parcel.obtain();
            try {
                record.unmarshall(Objects.requireNonNull(bytes), 0, bytes.length); record.setDataPosition(0);
                if (record.readInt() != 2) throw new IllegalStateException("native boot state record version differs");
                boolean first = record.readBoolean(), upgrade = record.readBoolean(), cross = record.readBoolean();
                boolean safe = record.readBoolean();String platform = Objects.requireNonNull(record.readString(),"admitted platform package unavailable");
                int count = record.readInt();
                if (count < 0 || count > record.dataAvail() / 8) throw new IllegalStateException("invalid native web policy count");
                Map<Integer,Boolean> web = new HashMap<>();
                for (int i = 0; i < count; i++) { int user = record.readInt(); boolean disabled = record.readBoolean(); if (user < 0 || web.put(user, disabled) != null) throw new IllegalStateException("invalid native user web policy"); }
                if (record.dataAvail() != 0) throw new IllegalStateException("native boot state has trailing bytes");
                return new State(first, upgrade, cross,safe,platform, Map.copyOf(web));
            } finally { record.recycle(); }
        }
    }
    public static final int PREPARE_READY=1, STORAGE_LISTENERS=2, INSTALLER_READY=3,
            DEX_OPTIMIZER_READY=4, RECONCILE_APPS=5, PACKAGE_OBSERVERS=6,
            MODULE_INFO_READY=7, APPLY_STAGED=8, SCHEDULE_MAINTENANCE=9,
            UPGRADE_DEXOPT=10, FSTRIM=11;
    private static final class Construction implements NativePackageManagerService.Construction {
        private final Context context;
        private final Installer installer;
        private final IServiceHost host;
        private final IPackageBootSession boot;
        private final OriginalOwners originals;
        private final PackageBootstrapBridge bridge;
        private com.android.server.pm.NativePackageObserverOwner observers;
        private final PackageManagerTracedLock installLock;
        private final Object packagesLock = new Object();
        private final NativeSnapshotOwner snapshotOwner;
        private final PackagePolicyBridge policy;
        private final com.android.server.pm.NativeEarlyPackageUserOwner earlyUser;
        private NativePackageUserOwner user;
        private NativePackageManagerInternal registeredInternal;
        private PackageLocal registeredLocal;
        private PackageSnapshots.Store snapshots;
        private Function<PackageSnapshots.ComputerSnapshot,NativeComputer> computers;
        private boolean finished;
        private boolean shutdownOwnerInstalled;
        private boolean runtimeReady;
        private boolean aborted;
        private Lifecycle lifecycle;
        private NativePMMutationProducer mutations;
        Construction(Context context, Installer installer, IServiceHost host,
                IPackageBootSession boot, PackageBootstrapBridge bridge, OriginalOwners originals,PackageManagerTracedLock installLock) throws RemoteException {
            this.installLock=Objects.requireNonNull(installLock);
            this.context = Objects.requireNonNull(context); this.installer = Objects.requireNonNull(installer);
            this.host = Objects.requireNonNull(host); this.boot = boot; this.bridge=Objects.requireNonNull(bridge); this.originals = originals;
            snapshotOwner = new NativeSnapshotOwner(boot); policy = new PackagePolicyBridge(context);
            earlyUser = new com.android.server.pm.NativeEarlyPackageUserOwner(
                    NativePackageCapabilities.attach(boot.getUserOperations()));
        }
        private NativeComputer computer() {
            if (aborted) throw new IllegalStateException("native package constructor epoch closed");
            if (snapshots == null || computers == null) throw new IllegalStateException("native Computer requested before scan completion");
            return computers.apply(snapshots.computer());
        }
        @Override public PackageManagerTracedLock installLock() { return installLock; }
        @Override public Object packageLock() { return packagesLock; }
        @Override public com.android.server.pm.NativeUserManagerBridge.Owner userPackages() { return earlyUser; }
        @Override public synchronized NativePackageManagerService.Inputs finish(UserManagerService users) throws RemoteException {
            if (finished || aborted) throw new IllegalStateException("native boot session no longer constructing");
            // Original permission constructors retain this exact object but do
            // not query packages. Any early query explicitly rejects unready state.
            var internal = new NativePackageManagerInternal();
            if (com.android.server.LocalServices.getService(android.content.pm.PackageManagerInternal.class) != null)
                throw new IllegalStateException("foreign package internal owner already registered");
            com.android.server.LocalServices.addService(android.content.pm.PackageManagerInternal.class, internal);
            registeredInternal = internal;
            PermissionManagerServiceInternal permissionOwner = Objects.requireNonNull(originals.permissions().get());
            LegacyPermissionManagerInternal legacyOwner = Objects.requireNonNull(originals.legacy().get());
            boot.finishRawScan();
            boolean initialCrossUser=boot.getInitialCrossUserSuspensions();
            NativeInitialPackageSnapshot.Captured initial;
            try{initial=NativeInitialPackageSnapshot.capture(boot,initialCrossUser);}
            catch(IOException failure){throw new IllegalStateException("native initial metadata capture failed",failure);}
            com.android.server.pm.permission.LegacyPermissionSettings initialDefinitions;
            try{initialDefinitions=NativeLegacyPermissionSettings.create(initial.version(),boot.getInitialLegacyPermissionDefinitionsRecord());}
            catch(RemoteException|RuntimeException failure){initial.data().close();throw failure;}
            var local=new PackageLocal(initial.data(),PackageBootstrapBridge.sdkDataOwner(),PackageBootstrapBridge.signingOwner());
            internal.bindInitialQueries(boot,local,initialDefinitions);
            internal.bindInitialLegacyRuntime(new NativeInitialLegacyRuntimePermissions(boot));
            registerPreparedOwners(internal,local);
            com.android.server.pm.NativePackageOriginalOwners.initializeSystemOverlays(local);
            com.android.server.pm.NativePackageOriginalOwners.initializePermissionState(permissionOwner,initialDefinitions,
                    local,boot.getInitialPermissionAdmissionsRecord(),boot.getInitialDeviceUpgrading());
            boot.finishInitialScan();
            State state = State.read(boot.getBootstrapState());
            try {
                snapshots = new PackageSnapshots.Store(boot::getSnapshot, snapshotOwner, state.crossUser(),
                        new PackageVersionPage(Objects.requireNonNull(boot.getVersionPage())));
                snapshots.refresh();
            } catch (IOException failure) { throw new IllegalStateException("native full replica capture failed", failure); }
            computers = scope -> {
                try {
                    State captured = State.read(boot.getBootstrapStateForVersion(scope.getVersion()));
                    return NativePMInterfaceProducerGroups.computer(scope, scope.getVersion(),
                            captured.webDisabled(), captured.safeMode(),captured.platformPackage(),context,
                            Objects.requireNonNull(com.android.server.LocalServices.getService(com.android.server.pm.UserManagerInternal.class)),policy);
                } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            };
            bridge.installPackageShellReadOwner(context,snapshots);
            bridge.installInstallerPermissionOwner(snapshots, snapshotOwner, state.crossUser());
            IPackageInternalHost internalHost = NativePackageCapabilities.attach(boot.getInternalHost());
            var mutation = new NativePackageStateMutation(new NativePackageMutationPublisher(internalHost, snapshotOwner, state.crossUser()));
            var checksums = new com.android.server.pm.NativePackageChecksums(context, snapshots);
            var loading = new NativePackageLoadingProgress(context, snapshots);
            android.content.ComponentName instantResolver;
            try (var capture = snapshots.computer()) { instantResolver = capture.readQueries().getInstantAppResolverComponent(); }
            var instant = new com.android.server.pm.NativeInstantResolution(context, BackgroundThread.getHandler(),
                    users, instantResolver, this::computer);
            var resolverOwner=Objects.requireNonNull(originals.resolver().create(context,snapshots));
            var overlays = new NativeSystemOverlayOwner(context.getApplicationInfo(),resolverOwner.activity(),resolverOwner.replaced());
            mutations = new NativePMMutationProducer(internalHost, mutation, checksums, loading, instant, overlays);
            var runtime = new NativePMLegacyRuntimePermissions(internalHost);
            var reads = new NativeInitializedPermissionReads(initialDefinitions, runtime, originals.dynamicCode(), originals.archiver().create(snapshots,internalHost,this::computer), mutations);
            observers = new com.android.server.pm.NativePackageObserverOwner(snapshots);
            bridge.installPackageObserverOwner(observers);
            var policyGroup = new NativePMInterfaceProducerGroups.Policy(policy);
            internal.bind(snapshots, reads, mutations, mutations,
                    policyGroup, policyGroup, observers, computers, runtime);
            local.bindFull(snapshots);
            bridge.installPackageDomainSettings(this::computer, BackgroundThread.getHandler());
            // These producers need the real initial replica and original UM, but do
            // not fetch the full installer runtime that finishConstruction creates.
            bridge.installPackageMaintenanceOwner(snapshots, originals.artManager(), context,
                    installer, installLock, originals.parserCache());
            bridge.installPackageShutdownOwner(originals.compilerStats(), originals.dexManager(),
                    reads.getDynamicCodeLogger(), context);
            shutdownOwnerInstalled=true;
            bridge.installPackageMutationOwner(context, BackgroundThread.getHandler(),
                    this::computer, originals.effectsPolicy());
            String fingerprint = android.os.SystemProperties.get("ro.build.fingerprint");
            if (fingerprint == null || fingerprint.isEmpty()) throw new IllegalStateException("actual image fingerprint unavailable");
            user = new NativePackageUserOwner(context, installer, installLock, packagesLock,
                    NativePackageCapabilities.attach(boot.getUserOperations()), this::computer,
                    permissionOwner, legacyOwner, fingerprint);
            earlyUser.attachAfterScan(user);
            boot.finishConstruction();
            runtimeReady=true;
            lifecycle = new Lifecycle(boot, state, users, snapshots, mutations, observers);
            finished = true;
            return new NativePackageManagerService.Inputs(snapshots, internal, local, computers,
                    lifecycle, new NativePackageManagerService.OriginalArtOwner(new com.android.server.pm.NativePackageDexoptCompletion(context,originals.compilerStats(),packagesLock,NativePackageCapabilities.attach(boot.getDexoptCompletion()))),
                    user.permissionLifecycle(), android.os.Binder.allowBlocking(Objects.requireNonNull(boot.getPackageService())),
                    android.os.Binder.allowBlocking(Objects.requireNonNull(boot.getPackageNativeService())));
        }
        @Override public synchronized void abort(Throwable failure) {
            if (aborted) return;
            aborted = true;
            if (lifecycle != null) {
                try { lifecycle.shutdown(); } catch (RuntimeException cleanup) { failure.addSuppressed(cleanup); }
                try { removePreparedOwners(); } catch (RuntimeException cleanup) { failure.addSuppressed(cleanup); }
                return;
            }
            if (runtimeReady && shutdownOwnerInstalled && mutations != null) try { mutations.shutdown(); } catch (RuntimeException cleanup) { failure.addSuppressed(cleanup); }
            try { boot.close(); } catch (RemoteException | RuntimeException cleanup) { failure.addSuppressed(cleanup); }
            if (observers != null) try { observers.close(); } catch (RuntimeException cleanup) { failure.addSuppressed(cleanup); }
            if (snapshots != null) try { snapshots.close(); } catch (RuntimeException cleanup) { failure.addSuppressed(cleanup); }
            try { removePreparedOwners(); } catch (RuntimeException cleanup) { failure.addSuppressed(cleanup); }
        }
        private void registerPreparedOwners(NativePackageManagerInternal internal, PackageLocal local) {
            if (com.android.server.LocalServices.getService(android.content.pm.PackageManagerInternal.class) != internal
                    || com.android.server.LocalManagerRegistry.getManager(com.android.server.pm.PackageManagerLocal.class) != null)
                throw new IllegalStateException("foreign package local owners already registered");
            com.android.server.LocalManagerRegistry.addManager(com.android.server.pm.PackageManagerLocal.class, local);
            registeredLocal = local;
        }
        private void removePreparedOwners() {
            if (registeredLocal != null) registeredLocal.closeEpoch();
            if (registeredInternal != null) registeredInternal.closeEpoch();
            if (registeredInternal != null
                    && com.android.server.LocalServices.getService(android.content.pm.PackageManagerInternal.class) == registeredInternal)
                com.android.server.LocalServices.removeServiceForTest(android.content.pm.PackageManagerInternal.class);
            // Original LocalManagerRegistry has no removal operation. Startup
            // failure propagates and ends this SystemServer process; snapshots
            // have already closed, so the retained local facade cannot be used.
            registeredInternal = null;
            registeredLocal = null;
        }
    }
    private static final class Lifecycle implements NativePackageManagerService.Lifecycle {
        private final IPackageBootSession boot;
        private final State state;
        private final UserManagerService users;
        private final PackageSnapshots.Store snapshots;
        private final NativePMMutationProducer mutations;
        private final com.android.server.pm.NativePackageObserverOwner observers;
        private final FutureTask<Void> appData;
        private final Thread appDataThread;
        private boolean closed;
        Lifecycle(IPackageBootSession boot, State state, UserManagerService users,
                PackageSnapshots.Store snapshots, NativePMMutationProducer mutations, com.android.server.pm.NativePackageObserverOwner observers) {
            this.boot = boot; this.state = state; this.users = users;
            this.snapshots = snapshots; this.mutations = mutations; this.observers=observers;
            appData = new FutureTask<>(() -> { boot.waitForAppDataPrepared(); return null; });
            appDataThread = new Thread(appData, "NativePackageAppDataBarrier");
            appDataThread.start();
        }
        private void run(int stage) {
            try { boot.runLifecycle(stage); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }
        @Override public boolean isFirstBoot() { snapshots.getVersion(); return state.firstBoot(); }
        @Override public boolean isDeviceUpgrading() { snapshots.getVersion(); return state.upgrading(); }
        @Override public Future<?> appDataPreparation() { return appData; }
        @Override public void updatePackagesIfNeeded() { run(UPGRADE_DEXOPT); }
        @Override public void updateMetrics(android.util.DisplayMetrics metrics) {
            Parcel record = Parcel.obtain();
            try {
                record.writeInt(1); record.writeInt(metrics.widthPixels); record.writeInt(metrics.heightPixels);
                record.writeInt(metrics.densityDpi); record.writeFloat(metrics.density);
                record.writeFloat(metrics.scaledDensity); record.writeFloat(metrics.xdpi); record.writeFloat(metrics.ydpi);
                boot.updateMetrics(record.marshall());
            } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            finally { record.recycle(); }
        }
        @Override public void performFstrimIfNeeded() { run(FSTRIM); }
        @Override public void prepareSystemReady() { run(PREPARE_READY); }
        @Override public void registerStorageListeners() { run(STORAGE_LISTENERS); }
        @Override public void installerSystemReady() { run(INSTALLER_READY); }
        @Override public void dexOptimizerSystemReady() { run(DEX_OPTIMIZER_READY); }
        @Override public void reconcileInternalApps() { run(RECONCILE_APPS); }
        @Override public List<android.content.pm.UserInfo> livingUsers() { return users.getUsers(true, true, false); }
        @Override public void registerPackageObservers() { run(PACKAGE_OBSERVERS); }
        @Override public void moduleInfoSystemReady() { run(MODULE_INFO_READY); }
        @Override public void restoreAndApplyStagedSessions() { run(APPLY_STAGED); }
        @Override public void schedulePostReadyMaintenance() { run(SCHEDULE_MAINTENANCE); }
        @Override public void shutdown() {
            synchronized (this) { if (closed) return; closed = true; }
            RuntimeException failure = null;
            try { mutations.shutdown(); } catch (RuntimeException error) { failure = error; }
            try { boot.close(); }
            catch (RemoteException error) { failure = combine(failure, error.rethrowFromSystemServer()); }
            catch (RuntimeException error) { failure = combine(failure, error); }
            appData.cancel(true);
            appDataThread.interrupt();
            try { observers.close(); } catch (RuntimeException error) { failure = combine(failure, error); }
            try { snapshots.close(); } catch (RuntimeException error) { failure = combine(failure, error); }
            if (failure != null) throw failure;
        }
        private static RuntimeException combine(RuntimeException failure, RuntimeException next) {
            if (failure == null) return next;
            failure.addSuppressed(next);
            return failure;
        }
    }
}
