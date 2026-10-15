package dev.aim.server;
import android.content.pm.IPackageInstallerSessionFileSystemConnector;
import android.content.pm.InstallationFileParcel;
import android.os.IRemoteCallback;
interface IInstallerPreparationBridge {
    boolean supportsCheckpoint();
    int getRollbackId(int rootId, boolean enableRollback, boolean rollback);
    void startCheckpoint();
    void submitApex(int rootId, in int[] apexChildren, boolean rollback, int rollbackId);
    void markApexReady(int rootId);
    boolean abortApex(int rootId);
    void markApexSuccessful(int rootId);
    boolean prepareStreaming(int sessionId, in byte[] params, in InstallationFileParcel[] added,
        in String[] removed, IPackageInstallerSessionFileSystemConnector connector,
        IRemoteCallback status);
    boolean prepareIncremental(int sessionId, String stagePath, String inheritedPath,
        in byte[] params, in InstallationFileParcel[] added, in byte[] readTimeouts,
        IRemoteCallback status);
    void destroyStreaming(int sessionId);
    byte[] getIncrementalInputs(String packageName, int userId);
}
