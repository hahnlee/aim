package dev.aim.server;
import android.content.IntentSender;
import android.os.IRemoteCallback;
import android.content.pm.PackageInstaller.PreapprovalDetails;
import android.graphics.Bitmap;
/** Original AM/app-state and IntentSender owners for native installer callbacks. */
interface IInstallerExternalBridge {
    IBinder getRemovalBridge();
    int getConstraintAppState(in List<String> packages, int flags, boolean deviceIdle);
    List<String> getConstraintDependencyPackages(in List<String> packages);
    oneway void watchAppState(IRemoteCallback callback);
    oneway void requestIdleJob(IRemoteCallback callback);
    boolean isIntentSenderImmutable(in IntentSender receiver);
    boolean isCommitMutableReceiverEnforced(int uid);
    boolean isSecureFrpActive();
    int getUserActionPolicy(String installer, int installerUid, int userId);
    boolean isSilentInstallTargetAllowed(String packageName, int targetSdk);
    boolean isPreapprovalRequestAvailable();
    long getAppMetadataSizeLimit();
    byte[] normalizeInstallerIcon(in Bitmap icon);
    /** Decode full original SDK presentation (styled labels/FD icons); native owns session policy. */
    String preparePreapproval(int sessionId, in PreapprovalDetails details, in IntentSender receiver);
    void sendPreapprovalStatus(int sessionId, int legacyStatus, String message, String pendingInstallerPackage, boolean preapprovalExtra);
    void clearPreapproval(int sessionId);
    void sendSessionStatus(in IntentSender receiver, int sessionId, String packageName,
        int legacyStatus, String message, boolean preapproval, String pendingInstallerPackage);
    void sendConstraintCallback(IRemoteCallback callback, boolean satisfied);
    void sendConstraintIntent(in IntentSender callback, in List<String> packages,
        int constraintFlags, boolean satisfied);
    void close();
    void sendPendingStreaming(in IntentSender receiver,int sessionId,String message);
    /** Original guest kernel/user/image configuration, no host defaults. */
    byte[] getNativeInstallEnvironment();
    int getZipLocalUtcOffset(int year, int month, int day, int hour, int minute, int second);
}
