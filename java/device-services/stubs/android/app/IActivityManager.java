// Compile-only original AM Binder interface.
package android.app;
public interface IActivityManager extends android.os.IInterface {
    int checkPermission(String permission, int pid, int uid) throws android.os.RemoteException;
    void killApplication(String packageName, int appId, int userId, String reason, int exitReason) throws android.os.RemoteException;
    int[] getRunningUserIds() throws android.os.RemoteException;
    int broadcastIntentWithFeature(IApplicationThread caller, String featureId, android.content.Intent intent, String resolvedType, android.content.IIntentReceiver resultTo, int resultCode, String resultData, android.os.Bundle resultExtras, String[] requiredPermissions, String[] excludePermissions, String[] excludePackages, int appOp, android.os.Bundle options, boolean serialized, boolean sticky, int userId) throws android.os.RemoteException;
}
