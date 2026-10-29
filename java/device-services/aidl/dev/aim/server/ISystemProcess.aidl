package dev.aim.server;
import dev.aim.server.IForegroundServicePolicy;
import dev.aim.server.IBatteryHealthStats;
import dev.aim.server.ISafeLabels;
import dev.aim.server.IStatsCompanionUidMap;
import dev.aim.server.IRecoverableKeyStoreKeys;
import dev.aim.server.IActivityStatsSnapshot;
import dev.aim.server.IActivityStatsHost;
import dev.aim.server.IStorageCrates;

/**
 * The app host's part of system_server for its system process
 * (docs/app-host.md, "The system process"): the app host registers it as
 * `aim.system_process`, and `aim-system` (SystemProcessHost), a process of
 * the system uid that runs `ActivityThread.systemMain` as system_server
 * does, calls it. It answers the system uid only.
 */
interface ISystemProcess {
    /**
     * `ActivityManagerService.setSystemProcess`: `applicationThread`, the
     * caller's IApplicationThread, is the system process's. The app host
     * runs the components declared in `android:process="system"` of the
     * system uid there and installs their providers in it
     * (`installSystemProviders`), as it runs an app's in an app's process.
     */
    void attachSystemProcess(IBinder applicationThread, IForegroundServicePolicy foregroundServicePolicy,
            ISafeLabels safeLabels, IBatteryHealthStats batteryHealthStats);

    /**
     * TelecomLoaderService's `InternalServiceRepository` (an
     * IInternalServiceRetriever), over the app host's DeviceIdleInternal.
     */
    IBinder getInternalServiceRepository();

    /** Original Java callback registry for the native connectivity owner (#1509). */
    void attachNetworkActivityBridge(IBinder bridge);
    /**
     * Attach the retained cold StatsCompanionService UID-map/receiver owner.
     * Replays the current statsd after a system-process restart; system UID only.
     */
    void attachStatsCompanionUidMap(IStatsCompanionUidMap uidMap);
    /** System-UID-only backend and original front end for domain verification (#1763). */
    IBinder getDomainVerificationBackend();
    void attachDomainVerification(IBinder frontend);
    boolean isDomainVerifier(int uid);
    /** ActiveModeWarden's cold listener registry and empty Passpoint query (#2261). */
    void attachWifiBridge(IBinder bridge);
    /** Original Java policy default; system UID only. */
    void attachDevicePolicyDefaults(IBinder defaults);
    /** Attach the cold original recoverable-key owner; system UID only (#1888). */
    void attachRecoverableKeyStoreKeys(IRecoverableKeyStoreKeys keys);
    /** Retained MediaRouter2 system route/session queries; system UID only. */
    void attachSystemRouteQueries(IBinder queries);
    /** Original ContentValues scalar string conversion; system UID only. */
    void attachSettingsValues(IBinder bridge);
    IActivityStatsHost getActivityStatsHost(IActivityStatsSnapshot snapshot);
    /** Release a diagnostics listener's shared native request count; system UID only. */
    void releaseConnectivityDiagnosticsRequest(int requestId);
    /** Original clone-provider query policy; system UID only. */
    void attachProviderQueries(IBinder queries);
    /** Original Installer crate queries and CrateInfo conversion; system UID only. */
    void attachStorageCrates(IStorageCrates crates);
    /** Retained package helpers; system UID only, before setSystemProcess. */
    void attachPackageBootstrapJava(IBinder bridge);
    /** Local permission/storage adapters in aim-system; system UID only. */
    void existingPackagePermissionsInstalled(String packageName, in String[] allowlisted, int userId);
    boolean isPackageCeStoragePrepared(int userId);
    /** PermissionManagerServiceInternal's per-user reset; system UID only. */
    void resetPackageRuntimePermissionsForUser(int userId);
    /** Original ApexManager staged inventory; system UID only. */
    void attachNativeStagingBridge(IBinder bridge);
    /** Original audio policy configuration and sync-adapter discovery; system UID only. */
    void attachAudioPolicyQueries(IBinder queries);
    void attachSyncAdapterQueries(IBinder queries);
    /** PackageManagerInternal.filterAppAccess at the native package owner. */
    boolean filterSyncAdapterPackageAccess(String packageName, int uid, int userId);
    /** Synchronous write of the live native CompilerStats; system UID only. */
    void writePackageCompilerStats();
    /** Initial camera privacy allowlist from original SystemConfig; system UID only. */
    void initializeCameraPrivacyAllowlist(in List<String> packages);
    /** Original UserManagerService user-switcher policy; system UID only. */
    void attachUserSwitcherPolicy(IBinder policy);
    /** Original Intent command parser; system UID only. */
    void attachPackageIntentParser(IBinder parser);
}
