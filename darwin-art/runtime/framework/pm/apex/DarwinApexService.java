package dev.darwinart.runtime.pm.apex;

import android.apex.ApexInfo;
import android.apex.ApexInfoList;
import android.apex.ApexSessionInfo;
import android.apex.ApexSessionParams;
import android.apex.CompressedApexInfoList;
import android.apex.IApexService;
import android.os.ServiceSpecificException;
import com.android.apex.XmlParser;
import java.io.File;
import java.io.FileInputStream;
import java.io.InputStream;
import java.util.List;

/**
 * The {@code apexservice} (apexd) binder for this runtime (ADR 0009).
 *
 * <p>The APEXes are activated when the system image is assembled, not by a
 * running apexd, and the image carries apexd's own record of them,
 * {@code /apex/apex-info-list.xml}. Queries are answered from that record
 * through AOSP's generated parser. There is no staging or APEX update path, so
 * session and install calls fail rather than pretend to succeed.</p>
 */
public final class DarwinApexService extends IApexService.Stub {
    public static final String SERVICE_NAME = "apexservice";
    private static final int EOPNOTSUPP = 95;
    private final ApexInfo[] active;

    public DarwinApexService(File apexInfoList, File apexRoot) throws Exception {
        List<com.android.apex.ApexInfo> recorded;
        try (InputStream input = new FileInputStream(apexInfoList)) {
            recorded = XmlParser.readApexInfoList(input).getApexInfo();
        }
        active = new ApexInfo[recorded.size()];
        for (int i = 0; i < active.length; i++) {
            com.android.apex.ApexInfo source = recorded.get(i);
            ApexInfo info = new ApexInfo();
            info.moduleName = source.getModuleName();
            info.modulePath = source.getModulePath();
            info.preinstalledModulePath = source.getPreinstalledModulePath();
            info.versionCode = source.getVersionCode();
            info.versionName = source.getVersionName();
            info.isFactory = source.getIsFactory();
            info.isActive = source.getIsActive();
            info.partition = partition(source.getPartition());
            info.hasClassPathJars = new File(apexRoot, info.moduleName + "/javalib").isDirectory();
            active[i] = info;
        }
    }

    private static byte partition(String name) {
        switch (name == null ? "SYSTEM" : name) {
            case "SYSTEM": return ApexInfo.Partition.SYSTEM;
            case "SYSTEM_EXT": return ApexInfo.Partition.SYSTEM_EXT;
            case "PRODUCT": return ApexInfo.Partition.PRODUCT;
            case "VENDOR": return ApexInfo.Partition.VENDOR;
            case "ODM": return ApexInfo.Partition.ODM;
            default: throw new IllegalArgumentException("unknown APEX partition " + name);
        }
    }

    private static ServiceSpecificException unsupported(String operation) {
        return new ServiceSpecificException(EOPNOTSUPP,
                "apexd." + operation + ": APEX updates are not supported by this runtime");
    }

    @Override public ApexInfo[] getActivePackages() { return active.clone(); }

    /** Only the factory versions exist, and all of them are active. */
    @Override public ApexInfo[] getAllPackages() { return active.clone(); }

    /** No staged APEX sessions can exist. */
    @Override public ApexSessionInfo[] getSessions() { return new ApexSessionInfo[0]; }

    @Override public void submitStagedSession(ApexSessionParams params, ApexInfoList list) {
        throw unsupported("submitStagedSession");
    }
    @Override public void markStagedSessionReady(int sessionId) {
        throw unsupported("markStagedSessionReady");
    }
    @Override public void markStagedSessionSuccessful(int sessionId) {
        throw unsupported("markStagedSessionSuccessful");
    }
    @Override public ApexSessionInfo getStagedSessionInfo(int sessionId) {
        throw unsupported("getStagedSessionInfo");
    }
    @Override public ApexInfo[] getStagedApexInfos(ApexSessionParams params) {
        throw unsupported("getStagedApexInfos");
    }
    @Override public void abortStagedSession(int sessionId) {
        throw unsupported("abortStagedSession");
    }
    @Override public void revertActiveSessions() { throw unsupported("revertActiveSessions"); }
    @Override public void resumeRevertIfNeeded() { throw unsupported("resumeRevertIfNeeded"); }
    /**
     * apexd marks the sessions activated on this boot successful; no staged
     * session can exist here, so the boot completes with none to mark.
     */
    @Override public void markBootCompleted() {}
    @Override public void snapshotCeData(int userId, int rollbackId, String apexName) {
        throw unsupported("snapshotCeData");
    }
    @Override public void restoreCeData(int userId, int rollbackId, String apexName) {
        throw unsupported("restoreCeData");
    }
    @Override public void destroyDeSnapshots(int rollbackId) {
        throw unsupported("destroyDeSnapshots");
    }
    @Override public void destroyCeSnapshots(int userId, int rollbackId) {
        throw unsupported("destroyCeSnapshots");
    }
    @Override public void destroyCeSnapshotsNotSpecified(int userId, int[] retainRollbackIds) {
        throw unsupported("destroyCeSnapshotsNotSpecified");
    }
    @Override public void unstagePackages(List<String> activePackagePaths) {
        throw unsupported("unstagePackages");
    }
    @Override public ApexInfo installAndActivatePackage(String packagePath, boolean force) {
        throw unsupported("installAndActivatePackage");
    }
    @Override public void stagePackages(List<String> packageTmpPaths) {
        throw unsupported("stagePackages");
    }
    @Override public long calculateSizeForCompressedApex(CompressedApexInfoList infoList) {
        throw unsupported("calculateSizeForCompressedApex");
    }
    @Override public void reserveSpaceForCompressedApex(CompressedApexInfoList infoList) {
        throw unsupported("reserveSpaceForCompressedApex");
    }
    @Override public void recollectPreinstalledData() {
        throw unsupported("recollectPreinstalledData");
    }
}
