package dev.aim.server;
/** Original shell-only property, permission classification and reset owners. */
import android.os.ParcelFileDescriptor;
interface IPackageShellPolicyBridge {
    boolean bootCompleted();
    void resetRuntimePermissions(int callerPid, int callerUid);
    String[] runtimePermissions(in String[] requested);
    int handleCreateUserCommand(in ParcelFileDescriptor input, in ParcelFileDescriptor output,
        in ParcelFileDescriptor error, in String[] arguments);
}
