package dev.aim.server;
import dev.aim.server.IForegroundServicePolicy;
import dev.aim.server.IBatteryHealthStats;
import dev.aim.server.ISafeLabels;
import dev.aim.server.IStatsCompanionUidMap;

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
}
