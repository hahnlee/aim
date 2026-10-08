package dev.aim.server;
/** Original shell-only property, permission classification and reset owners. */
interface IPackageShellPolicyBridge {
    boolean bootCompleted();
    void resetRuntimePermissions(int callerPid, int callerUid);
    String[] runtimePermissions(in String[] requested);
}
