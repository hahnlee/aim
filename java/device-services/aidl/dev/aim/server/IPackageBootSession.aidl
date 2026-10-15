package dev.aim.server;
import dev.aim.server.IPackageInternalHost;
import dev.aim.server.IPackageDexoptCompletion;
import dev.aim.server.IPackageUserOperations;
import dev.aim.server.IPackageScanSnapshot;
import android.os.ParcelFileDescriptor;
/** One actual native C constructor, before and after original UM publication. */
interface IPackageBootSession {
    IPackageInternalHost getInternalHost();
    IPackageUserOperations getUserOperations();
    /** Native raw scan with actual settings/users/visibility, before live permission projection. */
    void finishRawScan();
    String getInitialInstantAppPackageName(int uid);
    boolean filterInitialPackageAccess(String packageName, int callingUid, int userId, boolean filterUninstalled);
    boolean filterInitialUidAccess(int uid, int callingUid);
    /** Admit initial code and publish only the boot capture capabilities. */
    void finishInitialScan();
    /** Configure complete runtime only after all Java bridge producers are attached. */
    void finishConstruction();
    IPackageScanSnapshot getSnapshot();
    ParcelFileDescriptor getVersionPage();
    IBinder getPackageService();
    IBinder getPackageNativeService();
    byte[] getBootstrapState();
    /** Same record, from the retained graph identified by version; reject retired epochs. */
    byte[] getBootstrapStateForVersion(long version);
    String filterPackageName(long version, String packageName, int callingUid, int userId);
    boolean shouldFilter(long version, String packageName, int callingUid, int userId);
    void runLifecycle(int stage);
    void updateMetrics(in byte[] metricsRecord);
    void waitForAppDataPrepared();
    IPackageDexoptCompletion getDexoptCompletion();
    void close();
    /** Real raw package metadata only; live permission/Computer queries reject. */
    IPackageScanSnapshot getInitialMetadataSnapshot();
    /** Constructor roles and legacy definitions from the same raw native owner. */
    String[] getInitialKnownPackageNames(int kind, int userId);
    byte[] getInitialLegacyPermissionDefinitionsRecord();
    boolean getInitialCrossUserSuspensions();
    /** Original runtime metadata envelope from the retained raw constructor. */
    byte[] getInitialLegacyRuntimePermissionsStateRecord(int userId);
    int getInitialLegacyPermissionsVersion(int userId);
    boolean getInitialDeviceUpgrading();
    /** Ordered native scan admissions, including actual nullable prior code. */
    byte[] getInitialPermissionAdmissionsRecord();
}
